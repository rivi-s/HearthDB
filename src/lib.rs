#![allow(non_snake_case)]

mod async_worker;
mod compat;
mod db;
mod delivery;
mod trace;
mod lua;
mod offsets;

use minhook::MinHook;
use std::ffi::c_void;
use std::sync::{Once, OnceLock};

type LoadScriptFunctionsT = unsafe extern "stdcall" fn();

static ORIG_PLAYER_LOAD: OnceLock<LoadScriptFunctionsT> = OnceLock::new();
static ORIG_GLUE_LOAD:   OnceLock<LoadScriptFunctionsT> = OnceLock::new();

unsafe fn register_hdb_functions() {
    db::reset_all();
    lua::register_lua_function("HDB_Open",      db::script_hdb_open       as *mut usize);
    lua::register_lua_function("HDB_Close",     db::script_hdb_close      as *mut usize);
    lua::register_lua_function("HDB_Execute",   db::script_hdb_execute    as *mut usize);
    lua::register_lua_function("HDB_Query",     db::script_hdb_query      as *mut usize);
    lua::register_lua_function("HDB_QueryRaw",  db::script_hdb_query_raw  as *mut usize);
    lua::register_lua_function("HDB_OpenAddon",  db::script_hdb_open_addon  as *mut usize);
    lua::register_lua_function("HDB_GetVersion",     db::script_hdb_get_version     as *mut usize);
    lua::register_lua_function("HDB_ExecuteAsync",   db::script_hdb_execute_async   as *mut usize);
    lua::register_lua_function("HDB_QueryAsync",     db::script_hdb_query_async     as *mut usize);
    lua::register_lua_function("HDB_QueryRawAsync",  db::script_hdb_query_raw_async as *mut usize);
    lua::register_lua_function("HDB_GetResult",      db::script_hdb_get_result      as *mut usize);
    lua::register_lua_function("HDB_ClearPoison",   db::script_hdb_clear_poison    as *mut usize);
    lua::register_lua_function("HDB_SetDeliveryProbe", db::script_hdb_delivery_probe as *mut usize);
    lua::register_lua_function("HDB_DumpDeliveryDiagnostics", db::script_hdb_delivery_dump as *mut usize);
    lua::register_lua_function("HDB_GetDeliveryStats", db::script_hdb_delivery_stats as *mut usize);
    lua::register_lua_function("HDB_Pump",          db::script_hdb_pump            as *mut usize);

    lua::lua_call_string(
        "if gcinfo then HDB_SetDeliveryProbe(gcinfo) end; CreateFrame('Frame', 'HDB_CallbackFrame'):SetScript('OnUpdate', HDB_Pump)",
        "HearthDB",
    );
}

unsafe extern "stdcall" fn player_load_hook() {
    if let Some(orig) = ORIG_PLAYER_LOAD.get() {
        orig();
    }
    register_hdb_functions();
}

unsafe extern "stdcall" fn glue_load_hook() {
    if let Some(orig) = ORIG_GLUE_LOAD.get() {
        orig();
    }
    register_hdb_functions();
}

/// Runs once, from `Load` rather than `DllMain` — we have to hook *after*
/// ClassicAPI so we chain onto its detour instead of clobbering it (see
/// [`compat`]), and enabling a hook freezes every thread, which is not
/// something to do under the loader lock.
unsafe fn install_hooks() {
    let classicapi = compat::ensure_installed();

    // Sampled before we patch anything.
    let player_chained = compat::is_detoured(offsets::PLAYER_LOAD_SCRIPT_FUNCTIONS);
    let glue_chained = compat::is_detoured(offsets::GLUE_LOAD_SCRIPT_FUNCTIONS);

    let player_hooked = create_hook(
        offsets::PLAYER_LOAD_SCRIPT_FUNCTIONS,
        player_load_hook as *mut _,
        &ORIG_PLAYER_LOAD,
    );
    let glue_hooked = create_hook(
        offsets::GLUE_LOAD_SCRIPT_FUNCTIONS,
        glue_load_hook as *mut _,
        &ORIG_GLUE_LOAD,
    );

    // Both trampolines are stored by now, so neither detour can fire with an
    // unset ORIG_*. One thread-freeze activates the pair.
    let enabled = MinHook::enable_all_hooks().is_ok();

    log_install(
        classicapi,
        [
            (
                "PlayerLoadScriptFunctions",
                offsets::PLAYER_LOAD_SCRIPT_FUNCTIONS,
                player_hooked,
                player_chained,
            ),
            (
                "GlueLoadScriptFunctions",
                offsets::GLUE_LOAD_SCRIPT_FUNCTIONS,
                glue_hooked,
                glue_chained,
            ),
        ],
        enabled,
    );
}

unsafe fn create_hook(
    target: usize,
    detour: *mut c_void,
    orig: &OnceLock<LoadScriptFunctionsT>,
) -> bool {
    match MinHook::create_hook(target as *mut _, detour) {
        Ok(trampoline) => {
            let _ = orig.set(std::mem::transmute(trampoline));
            true
        }
        Err(_) => false,
    }
}

/// Rewritten each launch. Losing the hook race is invisible from Lua — the
/// symptom is some *other* plugin's globals going missing — so leave a note.
fn log_install(
    classicapi: compat::ClassicApi,
    hooks: [(&str, usize, bool, bool); 2],
    enabled: bool,
) {
    let mut report = format!(
        "HearthDB {} hook install\nClassicAPI: {}\nenable_all_hooks: {}\n",
        env!("CARGO_PKG_VERSION"),
        classicapi.as_str(),
        if enabled { "ok" } else { "FAILED" },
    );
    for (name, address, hooked, chained) in hooks {
        report.push_str(&format!(
            "{} {:#010X}: {}, {}\n",
            name,
            address,
            if hooked { "hooked" } else { "CREATE FAILED" },
            if chained {
                "chained onto existing detour"
            } else {
                "first detour"
            },
        ));
    }

    let _ = std::fs::create_dir("Logs");
    let _ = std::fs::write("Logs/hdb_compat.log", report);
}

/// Deliberately empty; everything happens in `Load`.
#[no_mangle]
pub unsafe extern "system" fn DllMain(
    _hinstance: *mut c_void,
    _reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    1
}

/// Called by the plugin loader on the main thread once every dlls.txt DLL has
/// been injected. Idempotent, so a second caller costs nothing.
#[no_mangle]
pub extern "C" fn Load() -> u32 {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| unsafe { install_hooks() });
    0
}
