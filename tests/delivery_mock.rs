// Host-side execution of the actual delivery module against a modeled Lua stack.
#![allow(dead_code)]
use std::{cell::RefCell,collections::{BTreeMap,VecDeque},rc::Rc};
#[derive(Clone,Debug)] enum V {Nil,Str(String),Num(f64),Probe,Table(Rc<RefCell<BTreeMap<String,V>>>),Func}
thread_local! {static STACK:RefCell<Vec<V>>=const{RefCell::new(Vec::new())};static REFS:RefCell<BTreeMap<i32,V>>=const{RefCell::new(BTreeMap::new())};static CALLS:RefCell<Vec<Vec<V>>>=const{RefCell::new(Vec::new())};static READY:RefCell<VecDeque<(u64,async_worker::AsyncResult,async_worker::PendingCallback)>>=const{RefCell::new(VecDeque::new())};static REENTER:RefCell<bool>=const{RefCell::new(false)};static FAIL:RefCell<bool>=const{RefCell::new(false)};static SLOWROW:RefCell<bool>=const{RefCell::new(false)};static MEMORY:RefCell<f64>=const{RefCell::new(1000.0)};}
mod lua {
 use super::*;pub type LuaState=usize;pub const LUA_REGISTRYINDEX:i32=-10000;
 pub unsafe fn get_lua_state()->usize{0}
 pub unsafe fn lua_gettop(_:usize)->i32{STACK.with(|s|s.borrow().len() as i32)}
 pub unsafe fn lua_settop(_:usize,n:i32){STACK.with(|s|s.borrow_mut().truncate(n as usize));}
 pub unsafe fn lua_newtable(_:usize){if STACK.with(|s|matches!(s.borrow().last(),Some(V::Table(_)))) && SLOWROW.with(|s|{let v=*s.borrow();*s.borrow_mut()=false;v}) {std::thread::sleep(std::time::Duration::from_millis(12));MEMORY.with(|m|*m.borrow_mut()=800.0);}STACK.with(|s|s.borrow_mut().push(V::Table(Rc::new(RefCell::new(BTreeMap::new())))));}
 pub unsafe fn lua_pushstring(_:usize,v:&str){STACK.with(|s|s.borrow_mut().push(V::Str(v.into())));}
 pub unsafe fn lua_pushnumber(_:usize,v:f64){STACK.with(|s|s.borrow_mut().push(V::Num(v)));}
 pub unsafe fn lua_pushnil(_:usize){STACK.with(|s|s.borrow_mut().push(V::Nil));}
 pub unsafe fn lual_ref(_:usize,_:i32)->i32{let v=STACK.with(|s|s.borrow_mut().pop().unwrap());REFS.with(|r|{let mut r=r.borrow_mut();let n=r.keys().max().copied().unwrap_or(0)+1;r.insert(n,v);n})}
 pub unsafe fn lual_unref(_:usize,_:i32,r:i32){assert!(REFS.with(|v|v.borrow_mut().remove(&r)).is_some(),"double unref");}
 pub unsafe fn lua_rawgeti(_:usize,_:i32,r:i32){let v=REFS.with(|v|v.borrow().get(&r).expect("invalid ref").clone());STACK.with(|s|s.borrow_mut().push(v));}
 fn table_at(s:&[V],idx:i32)->Rc<RefCell<BTreeMap<String,V>>>{let pos=if idx<0{(s.len() as i32+idx) as usize}else{idx as usize-1};match &s[pos]{V::Table(t)=>t.clone(),_=>panic!("not table")}}
 pub unsafe fn lua_rawseti(_:usize,idx:i32,key:i32){STACK.with(|s|{let mut s=s.borrow_mut();let t=table_at(&s,idx);let v=s.pop().unwrap();if !matches!(v,V::Nil){t.borrow_mut().insert(key.to_string(),v);}});}
 pub unsafe fn lua_settable(_:usize,idx:i32){STACK.with(|s|{let mut s=s.borrow_mut();let t=table_at(&s,idx);let v=s.pop().unwrap();let k=match s.pop().unwrap(){V::Str(k)=>k,_=>panic!()};if !matches!(v,V::Nil){t.borrow_mut().insert(k,v);}});}
 pub unsafe fn lua_pcall(_:usize,n:i32,_:i32,_:i32)->i32{
 if n==0 && STACK.with(|s|matches!(s.borrow().last(),Some(V::Probe))) {STACK.with(|s|{let mut s=s.borrow_mut();s.pop();s.push(V::Num(MEMORY.with(|m|*m.borrow())));s.push(V::Num(2000.0));});return 0;}
 let args=STACK.with(|s|{let mut s=s.borrow_mut();let a=s.len()-n as usize;let args=s.split_off(a);assert!(matches!(s.pop(),Some(V::Func)));args});CALLS.with(|c|c.borrow_mut().push(args));if REENTER.with(|r|*r.borrow()){crate::delivery::pump();crate::delivery::cancel(None);}if FAIL.with(|f|*f.borrow()){lua_pushstring(0,"callback failed");1}else{0}}
 pub unsafe fn lua_isnumber(_:usize,idx:i32)->bool{STACK.with(|s|{let s=s.borrow();let i=if idx<0{(s.len() as i32+idx)as usize}else{idx as usize-1};matches!(s[i],V::Num(_))})}
 pub unsafe fn lua_tonumber(_:usize,idx:i32)->f64{STACK.with(|s|{let s=s.borrow();let i=if idx<0{(s.len() as i32+idx)as usize}else{idx as usize-1};match s[i]{V::Num(v)=>v,_=>panic!()}})}
 pub unsafe fn lua_tostring(_:usize,_:i32)->Option<String>{None}
}
mod async_worker {
 use super::*;pub enum AsyncResult{Execute{rows_affected:usize},Query{rows:Vec<Vec<(String,Option<String>)>>},QueryRaw{cols:Vec<String>,rows:Vec<Vec<Option<String>>>},Error{message:String}}
 pub enum CallbackType{Execute,Query,QueryRaw}
 pub struct PendingCallback{pub lua_ref:i32,pub cb_type:CallbackType,pub handle_id:usize}
 pub fn take_ready_callback()->Option<(u64,AsyncResult,PendingCallback)>{READY.with(|q|q.borrow_mut().pop_front())}
}
#[path="../src/trace.rs"] mod trace;
#[path="../src/delivery.rs"] mod delivery;
fn enqueue(result:async_worker::AsyncResult,kind:async_worker::CallbackType){let r=unsafe{STACK.with(|s|s.borrow_mut().push(V::Func));lua::lual_ref(0,0)};READY.with(|q|q.borrow_mut().push_back((1,result,async_worker::PendingCallback{lua_ref:r,cb_type:kind,handle_id:1})));}
fn raw(n:usize)->async_worker::AsyncResult{async_worker::AsyncResult::QueryRaw{cols:vec!["id".into(),"null".into(),"text".into()],rows:(0..n).map(|i|vec![Some(i.to_string()),None,Some("Привет".into())]).collect()}}
fn drain(){for _ in 0..1000{unsafe{delivery::pump()};assert_eq!(unsafe{lua::lua_gettop(0)},0);if REFS.with(|r|r.borrow().values().all(|v|matches!(v,V::Probe))){return}}panic!("not completed")}
fn table(v:&V)->Rc<RefCell<BTreeMap<String,V>>>{match v{V::Table(t)=>t.clone(),_=>panic!()}}
#[test]fn lifecycle(){use async_worker::*;
 STACK.with(|s|s.borrow_mut().push(V::Probe));unsafe{delivery::set_probe(lua::lual_ref(0,0));}
 enqueue(raw(11153),CallbackType::QueryRaw);unsafe{delivery::pump()};assert!(CALLS.with(|c|c.borrow().is_empty()));assert!(delivery::stats()[1]>0.0 && delivery::stats()[1]<=128.0);drain();
 CALLS.with(|c|{let c=c.borrow();assert_eq!(c.len(),1);assert_eq!(c[0].len(),3);let t=table(&c[0][1]);assert_eq!(t.borrow().len(),11153);for i in 1..=11153{let r=table(&t.borrow()[&i.to_string()]);let r=r.borrow();assert!(matches!(&r["1"],V::Str(v) if v==&(i-1).to_string()));assert!(!r.contains_key("2"));assert!(matches!(&r["3"],V::Str(v) if v=="Привет"));}});CALLS.with(|c|c.borrow_mut().clear());
 enqueue(raw(1000),CallbackType::QueryRaw);unsafe{delivery::pump();delivery::cancel(Some(2))};assert!(!REFS.with(|r|r.borrow().values().all(|v|matches!(v,V::Probe))));unsafe{delivery::cancel(Some(1))};assert!(REFS.with(|r|r.borrow().values().all(|v|matches!(v,V::Probe))));assert!(CALLS.with(|c|c.borrow().is_empty()));
 enqueue(raw(0),CallbackType::QueryRaw);drain();CALLS.with(|c|{let c=c.borrow();assert_eq!(table(&c[0][1]).borrow().len(),0)});CALLS.with(|c|c.borrow_mut().clear());
 enqueue(AsyncResult::Query{rows:vec![vec![("name".into(),Some("hello".into())),("null".into(),None)]]},CallbackType::Query);drain();CALLS.with(|c|{let c=c.borrow();assert_eq!(c[0].len(),2);let t=table(&c[0][0]);let r=table(&t.borrow()["1"]);assert_eq!(r.borrow().len(),1)});CALLS.with(|c|c.borrow_mut().clear());
 enqueue(AsyncResult::Error{message:"bad SQL".into()},CallbackType::QueryRaw);drain();CALLS.with(|c|{let c=c.borrow();assert_eq!(c[0].len(),3);assert!(matches!(&c[0][0],V::Nil));assert!(matches!(&c[0][1],V::Nil));assert!(matches!(&c[0][2],V::Str(v) if v=="bad SQL"));});CALLS.with(|c|c.borrow_mut().clear());
 REENTER.with(|r|*r.borrow_mut()=true);enqueue(raw(2),CallbackType::QueryRaw);enqueue(raw(2),CallbackType::QueryRaw);unsafe{delivery::pump()};assert_eq!(CALLS.with(|c|c.borrow().len()),1);drain();assert_eq!(CALLS.with(|c|c.borrow().len()),2);
 enqueue(raw(1000),CallbackType::QueryRaw);unsafe{delivery::pump();delivery::cancel(None)};assert!(REFS.with(|r|r.borrow().values().all(|v|matches!(v,V::Probe))));
 FAIL.with(|f|*f.borrow_mut()=true);enqueue(raw(1),CallbackType::QueryRaw);drain();FAIL.with(|f|*f.borrow_mut()=false);
 enqueue(AsyncResult::Execute{rows_affected:7},CallbackType::Execute);drain();
 REENTER.with(|r|*r.borrow_mut()=false);SLOWROW.with(|s|*s.borrow_mut()=true);enqueue(raw(2),CallbackType::QueryRaw);drain();let report=delivery::diagnostics();assert!(report.contains("WORST ticket="));assert!(report.contains("Some(1000.0)->Some(800.0)"));assert!(report.contains("maxRowMs="));
 delivery::reset_stats();assert_eq!(delivery::stats(),[0.0;5]);assert!(REFS.with(|r|r.borrow().is_empty()));
}
