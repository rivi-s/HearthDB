//! Experimental main-thread delivery. SQL runs once; Lua rows are built over
//! multiple pumps. The original callback still receives one complete result.
use crate::{
    async_worker::{self, AsyncResult, PendingCallback},
    lua,
};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const MAX_ROWS: usize = 128;
const MAX_CELLS: usize = 2048;
const SLICE: Duration = Duration::from_millis(2);
struct Delivery {
    ticket: u64,
    result: AsyncResult,
    cb: PendingCallback,
    next: usize,
    rows_ref: Option<i32>,
}
thread_local! {
    static PROBE: Cell<Option<i32>> = const { Cell::new(None) };
    static SLOW: RefCell<VecDeque<String>> = const { RefCell::new(VecDeque::new()) };
    static WORST: RefCell<Option<(f64,String)>> = const { RefCell::new(None) };
    static ACTIVE: RefCell<Option<Delivery>> = const { RefCell::new(None) };
    static STATS: Cell<[f64; 5]> = const { Cell::new([0.0; 5]) };
    static PUMPING: Cell<bool> = const { Cell::new(false) };
}
struct PumpGuard;
impl Drop for PumpGuard {
    fn drop(&mut self) {
        PUMPING.with(|p| p.set(false));
    }
}
impl Delivery {
    unsafe fn release(&mut self, l: lua::LuaState) {
        if let Some(r) = self.rows_ref.take() {
            lua::lual_unref(l, lua::LUA_REGISTRYINDEX, r);
        }
    }
    unsafe fn build(&mut self, l: lua::LuaState, start: Instant, work: &mut SliceWork) -> bool {
        let len = match &self.result {
            AsyncResult::Query { rows } => rows.len(),
            AsyncResult::QueryRaw { rows, .. } => rows.len(),
            _ => return true,
        };
        let base = lua::lua_gettop(l);
        let r = match self.rows_ref {
            Some(r) => r,
            None => {
                lua::lua_newtable(l);
                let r = lua::lual_ref(l, lua::LUA_REGISTRYINDEX);
                self.rows_ref = Some(r);
                r
            }
        };
        lua::lua_rawgeti(l, lua::LUA_REGISTRYINDEX, r);
        let mut count = 0;
        let mut cells = 0;
        while self.next < len {
            let row_start = Instant::now();
            lua::lua_newtable(l);
            match &mut self.result {
                AsyncResult::QueryRaw { rows, .. } => {
                    let row = std::mem::take(&mut rows[self.next]);
                    cells += row.len();
                    for (i, val) in row.iter().enumerate() {
                        match val {
                            Some(v) => lua::lua_pushstring(l, v),
                            None => lua::lua_pushnil(l),
                        }
                        lua::lua_rawseti(l, -2, (i + 1) as i32);
                    }
                }
                AsyncResult::Query { rows } => {
                    let row = std::mem::take(&mut rows[self.next]);
                    cells += row.len();
                    for (key, val) in &row {
                        lua::lua_pushstring(l, key);
                        match val {
                            Some(v) => lua::lua_pushstring(l, v),
                            None => lua::lua_pushnil(l),
                        }
                        lua::lua_settable(l, -3);
                    }
                }
                _ => unreachable!(),
            }
            self.next += 1;
            lua::lua_rawseti(l, -2, self.next as i32);
            count += 1;
            let row_ms = row_start.elapsed().as_secs_f64() * 1000.0;
            if row_ms > work.max_row_ms {
                work.max_row_ms = row_ms;
                work.slowest_row = self.next;
            }
            work.rows = count;
            work.cells = cells;
            // Check after each complete row. A single row or GC cycle can
            // exceed the time target; Lua calls cannot be preempted safely.
            if count >= MAX_ROWS || cells >= MAX_CELLS || start.elapsed() >= SLICE {
                break;
            }
        }
        lua::lua_settop(l, base);
        self.next == len
    }
    unsafe fn finish(mut self, l: lua::LuaState) {
        let base = lua::lua_gettop(l);
        lua::lua_rawgeti(l, lua::LUA_REGISTRYINDEX, self.cb.lua_ref);
        let nargs = match &self.result {
            AsyncResult::Execute { rows_affected } => {
                lua::lua_pushnumber(l, *rows_affected as f64);
                lua::lua_pushnil(l);
                2
            }
            AsyncResult::Query { .. } => {
                lua::lua_rawgeti(l, lua::LUA_REGISTRYINDEX, self.rows_ref.unwrap());
                lua::lua_pushnil(l);
                2
            }
            AsyncResult::QueryRaw { cols, .. } => {
                lua::lua_newtable(l);
                for (i, col) in cols.iter().enumerate() {
                    lua::lua_pushstring(l, col);
                    lua::lua_rawseti(l, -2, (i + 1) as i32);
                }
                lua::lua_rawgeti(l, lua::LUA_REGISTRYINDEX, self.rows_ref.unwrap());
                lua::lua_pushnil(l);
                3
            }
            AsyncResult::Error { message } => {
                lua::lua_pushnil(l);
                if matches!(self.cb.cb_type, async_worker::CallbackType::QueryRaw) {
                    lua::lua_pushnil(l);
                }
                lua::lua_pushstring(l, message);
                if matches!(self.cb.cb_type, async_worker::CallbackType::QueryRaw) {
                    3
                } else {
                    2
                }
            }
        };
        // Stack arguments keep tables alive across callback-driven close/reset.
        self.release(l);
        lua::lual_unref(l, lua::LUA_REGISTRYINDEX, self.cb.lua_ref);
        if lua::lua_pcall(l, nargs, 0, 0) != 0 {
            let err = lua::lua_tostring(l, -1).unwrap_or_else(|| "(non-string error)".into());
            let _ = std::fs::create_dir("Logs");
            use std::io::Write;
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("Logs/hdb_pump_errors.log")
                .and_then(|mut f| {
                    writeln!(
                        f,
                        "HDB_Pump: callback error (ticket {}): {}",
                        self.ticket, err
                    )
                });
        }
        lua::lua_settop(l, base);
    }
}
/// Called only on the game Lua thread. Never holds a RefCell borrow during Lua
/// callbacks, so callback-triggered close/reload and nested HDB_Pump are safe.
pub unsafe fn pump() {
    if PUMPING.with(|p| p.replace(true)) {
        return;
    }
    let _guard = PumpGuard;
    let active = ACTIVE.with(|a| a.borrow_mut().take());
    let mut delivery = match active.or_else(|| {
        async_worker::take_ready_callback().map(|(ticket, result, cb)| Delivery {
            ticket,
            result,
            cb,
            next: 0,
            rows_ref: None,
        })
    }) {
        Some(d) => d,
        None => return,
    };
    let l = lua::get_lua_state();
    let before = sample_memory(l);
    let previous = delivery.next;
    let mut work = SliceWork::default();
    let build_start = Instant::now();
    let complete = delivery.build(l, build_start, &mut work);
    let elapsed = build_start.elapsed().as_secs_f64() * 1000.0;
    crate::trace::slice(delivery.ticket, elapsed);
    let after = sample_memory(l);
    if elapsed >= 10.0 {
        let kind = match &delivery.result {
            AsyncResult::QueryRaw { .. } => "raw",
            AsyncResult::Query { .. } => "named",
            _ => "other",
        };
        let detail = format!("ticket={} handle={} kind={} startRow={} rows={} cells={} buildMs={:.3} maxRowMs={:.3} slowestRow={} memKB={:?}->{:?} thresholdKB={:?}->{:?}",
            delivery.ticket,delivery.cb.handle_id,kind,previous+1,work.rows,work.cells,elapsed,work.max_row_ms,work.slowest_row,before.map(|m|m.0),after.map(|m|m.0),before.and_then(|m|m.1),after.and_then(|m|m.1));
        WORST.with(|w| {
            let mut w = w.borrow_mut();
            if w.as_ref().is_none_or(|(ms, _)| elapsed > *ms) {
                *w = Some((elapsed, detail.clone()));
            }
        });
        SLOW.with(|q| {
            let mut q = q.borrow_mut();
            if q.len() == 32 {
                q.pop_front();
            }
            q.push_back(detail);
        });
    }
    STATS.with(|stats| {
        let mut s = stats.get();
        s[0] += 1.0;
        s[1] += (delivery.next - previous) as f64;
        s[3] = s[3].max(elapsed);
        stats.set(s);
    });
    if complete {
        let callback_start = Instant::now();
        let ticket = delivery.ticket;
        crate::trace::callback(ticket);
        delivery.finish(l);
        crate::trace::finish(ticket, "delivered");
        STATS.with(|stats| {
            let mut s = stats.get();
            s[2] += 1.0;
            s[4] = s[4].max(callback_start.elapsed().as_secs_f64() * 1000.0);
            stats.set(s);
        });
    } else {
        ACTIVE.with(|a| *a.borrow_mut() = Some(delivery));
    }
}
pub unsafe fn cancel(handle: Option<usize>) {
    crate::trace::cancel(handle);
    let active = ACTIVE.with(|a| {
        let mut a = a.borrow_mut();
        if a.as_ref()
            .is_some_and(|d| handle.is_none() || handle == Some(d.cb.handle_id))
        {
            a.take()
        } else {
            None
        }
    });
    if let Some(mut d) = active {
        let l = lua::get_lua_state();
        d.release(l);
        lua::lual_unref(l, lua::LUA_REGISTRYINDEX, d.cb.lua_ref);
    }
}

pub fn stats() -> [f64; 5] {
    STATS.with(|s| s.get())
}
pub fn reset_stats() {
    STATS.with(|s| s.set([0.0; 5]));
    SLOW.with(|q| q.borrow_mut().clear());
    WORST.with(|w| *w.borrow_mut() = None);
    // reset_all is called before functions are registered in the new session.
    // Use the same active Lua state cleanup convention as upstream callbacks.
    if let Some(r) = PROBE.with(|p| p.take()) {
        unsafe {
            lua::lual_unref(lua::get_lua_state(), lua::LUA_REGISTRYINDEX, r);
        }
    }
}

#[derive(Default)]
struct SliceWork {
    rows: usize,
    cells: usize,
    max_row_ms: f64,
    slowest_row: usize,
}
pub unsafe fn set_probe(r: i32) {
    if let Some(old) = PROBE.with(|p| p.replace(Some(r))) {
        lua::lual_unref(lua::get_lua_state(), lua::LUA_REGISTRYINDEX, old);
    }
}
unsafe fn sample_memory(l: lua::LuaState) -> Option<(f64, Option<f64>)> {
    let r = PROBE.with(|p| p.get())?;
    let base = lua::lua_gettop(l);
    lua::lua_rawgeti(l, lua::LUA_REGISTRYINDEX, r);
    let result = if lua::lua_pcall(l, 0, 2, 0) == 0 && lua::lua_isnumber(l, -2) {
        Some((
            lua::lua_tonumber(l, -2),
            if lua::lua_isnumber(l, -1) {
                Some(lua::lua_tonumber(l, -1))
            } else {
                None
            },
        ))
    } else {
        None
    };
    lua::lua_settop(l, base);
    result
}
pub fn diagnostics() -> String {
    let mut text=format!("HearthDB {} delivery diagnostics\nStats: {:?}\nMemory samples surround conversion only; net drops suggest collection, not proof.\n",env!("CARGO_PKG_VERSION"),stats());
    WORST.with(|w| {
        if let Some((_, line)) = &*w.borrow() {
            text.push_str("WORST ");
            text.push_str(line);
            text.push('\n');
        }
    });
    SLOW.with(|q| {
        for line in q.borrow().iter() {
            text.push_str(line);
            text.push('\n');
        }
    });
    text.push_str(&crate::trace::diagnostics());
    text
}
