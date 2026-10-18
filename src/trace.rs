//! Bounded in-memory timings. No SQL text, quest titles, or disk IO on hot paths.
use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;
const MAX_LIVE: usize = 2048;
const MAX_DONE: usize = 256;
#[derive(Clone)]
struct Timing {
    ticket: u64, handle: usize, label: String, submitted: Instant,
    worker: Option<Instant>, sql: Option<Instant>, ready: Option<Instant>,
    delivery: Option<Instant>, callback: Option<Instant>, done: Option<Instant>,
    rows: usize, build_ms: f64, slices: usize, status: &'static str,
}
#[derive(Default)]
struct Trace { live: HashMap<u64, Timing>, done: VecDeque<Timing> }
static TRACE: LazyLock<Mutex<Trace>> = LazyLock::new(|| Mutex::new(Trace::default()));
fn label(sql: &str) -> String {
    let s = sql.to_ascii_lowercase();
    if s.contains("select id from quest_text") { return "identity-title".into(); }
    if s.contains("lower(q.title) = lower(") && s.contains("quest_disambiguation") { return "identity-candidates".into(); }
    if s.contains("quest_disambiguation") { return "candidate-preload".into(); }
    if s.contains("select title, id from quest_text") { return "title-preload".into(); }
    if s.contains("with resolved_target as") {
        let id: String = s.split("where q.quest_id = ").nth(1).unwrap_or("").trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
        return format!("quest-target:q{}", id);
    }
    if s.contains("with resolved_start as") { return "quest-starters".into(); }
    if s.contains("meta_relation") { return "tracking".into(); }
    "other".into()
}
pub fn submit(ticket: u64, handle: usize, sql: &str) {
    let timing = Timing {ticket,handle,label:label(sql),submitted:Instant::now(),worker:None,sql:None,ready:None,delivery:None,callback:None,done:None,rows:0,build_ms:0.0,slices:0,status:"queued"};
    let mut t = TRACE.lock().unwrap();
    if t.live.len() >= MAX_LIVE { if let Some(id) = t.live.keys().min().copied() { t.live.remove(&id); } }
    t.live.insert(ticket,timing);
}
fn update(ticket: u64, f: impl FnOnce(&mut Timing)) { if let Some(t) = TRACE.lock().unwrap().live.get_mut(&ticket) { f(t); } }
pub fn worker(ticket:u64) { update(ticket,|t|{t.worker=Some(Instant::now());t.status="connection-wait";}); }
pub fn sql(ticket:u64) { update(ticket,|t|{t.sql=Some(Instant::now());t.status="sql";}); }
pub fn ready(ticket:u64, rows:usize) { update(ticket,|t|{t.ready=Some(Instant::now());t.rows=rows;t.status="ready";}); }
pub fn delivery(ticket:u64) { update(ticket,|t|{t.delivery=Some(Instant::now());t.status="converting";}); }
pub fn slice(ticket:u64, ms:f64) { update(ticket,|t|{t.build_ms+=ms;t.slices+=1;}); }
pub fn callback(ticket:u64) { update(ticket,|t|{t.callback=Some(Instant::now());t.status="callback";}); }
pub fn finish(ticket:u64, status:&'static str) {
    let mut t = TRACE.lock().unwrap();
    if let Some(mut record) = t.live.remove(&ticket) {
        record.done=Some(Instant::now());record.status=status;
        if t.done.len() == MAX_DONE { t.done.pop_front(); }
        t.done.push_back(record);
    }
}
pub fn cancel(handle:Option<usize>) {
    let ids:Vec<u64> = TRACE.lock().unwrap().live.iter().filter(|(_,t)|handle.is_none() || handle==Some(t.handle)).map(|(id,_)|*id).collect();
    for id in ids { finish(id,"cancelled"); }
}
pub fn reset() { *TRACE.lock().unwrap() = Trace::default(); }
fn ms(a:Option<Instant>, b:Option<Instant>) -> String { match (a,b) { (Some(a),Some(b)) => format!("{:.3}",b.saturating_duration_since(a).as_secs_f64()*1000.0), _=>"pending".into() } }
pub fn diagnostics() -> String {
    // Clone under lock, format after releasing it so the worker can continue.
    let mut rows:Vec<Timing> = { let t=TRACE.lock().unwrap();t.done.iter().chain(t.live.values()).cloned().collect() };
    rows.sort_by_key(|t|t.ticket);
    let mut out=String::from("Query timings (ms): queue=submit to worker; lock=connection wait; sql=execution/materialization; readyWait=result ready to first conversion; delivery=first conversion to callback (includes frame gaps); build=sum conversion work; callback=Lua callback. pending means a boundary was not reached.\n");
    for t in rows { out.push_str(&format!("ticket={} handle={} kind={} rows={} status={} queueMs={} lockMs={} sqlMs={} readyWaitMs={} deliveryMs={} buildMs={:.3} slices={} callbackMs={} totalMs={}\n",t.ticket,t.handle,t.label,t.rows,t.status,ms(Some(t.submitted),t.worker),ms(t.worker,t.sql),ms(t.sql,t.ready),ms(t.ready,t.delivery),ms(t.delivery,t.callback),t.build_ms,t.slices,ms(t.callback,t.done),ms(Some(t.submitted),t.done))); }
    out
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn lifecycle_and_privacy() {
        reset();submit(1,2,"SELECT id FROM quest_text WHERE title='private title'");worker(1);sql(1);ready(1,2);delivery(1);slice(1,0.5);callback(1);finish(1,"delivered");
        let s=diagnostics();assert!(s.contains("identity-title")&&s.contains("rows=2")&&s.contains("buildMs=0.500"));assert!(!s.contains("private"));
        submit(2,3,"x");cancel(Some(2));assert!(TRACE.lock().unwrap().live.contains_key(&2));cancel(Some(3));assert!(!TRACE.lock().unwrap().live.contains_key(&2));
        for id in 10..300 {submit(id,1,"x");finish(id,"polled");}assert_eq!(TRACE.lock().unwrap().done.len(),MAX_DONE);
        reset();assert!(!diagnostics().contains("ticket="));assert_eq!(label("WITH resolved_target AS (...) WHERE q.quest_id = 40211 ORDER BY 1"),"quest-target:q40211");
        for id in 1..=MAX_LIVE as u64+2 {submit(id,1,"x");}assert_eq!(TRACE.lock().unwrap().live.len(),MAX_LIVE);reset();
    }
}
