"""Re-enter production fast recalculation with the exact historical book/pool responses.
Only the depth gateway is replaced by already-priced quotes. The core cache, record eligibility,
risk hint, final calc budget/rounding, both thresholds, winner and fallback are unchanged.
"""
import sys
from pathlib import Path
root=Path(sys.argv[1])
def append(path,s):
 with (root/path).open('a') as f:f.write(s)
def rewrite(path,a,b):
 p=root/path;s=p.read_text();assert s.count(a)==1,(path,a[:60],s.count(a));p.write_text(s.replace(a,b))
append('src/core/final_depth.rs',"""
pub(crate) type Request=(Decimal,bool,tokio::sync::oneshot::Sender<Result<Calc,Fail>>);
tokio::task_local! {pub(crate) static LIVE_ARCHIVE:tokio::sync::mpsc::UnboundedSender<Request>;}
tokio::task_local! {pub(crate) static ARCHIVED: std::collections::BTreeMap<Decimal, Result<Calc, Fail>>;}
""")
rewrite('src/core/final_depth.rs','let src = Live { core, src: None };','''if let Ok(tx)=LIVE_ARCHIVE.try_with(Clone::clone){
        let (a,b)=tokio::sync::oneshot::channel();
        tx.send((qty,crate::gateway::cpu_budget::is_optional_quote(),a)).map_err(|_| Fail::Stop("archived adapter closed".into()))?;
        return b.await.map_err(|_|Fail::Stop("archived reply closed".into()))?;
    }
    if let Ok(r)=ARCHIVED.try_with(|map|map.get(&qty).cloned().unwrap_or_else(||Err(Fail::Stop("quantity not in archived response set".into())))){return r;}
    let src = Live { core, src: None };''')
append('src/core/precheck_hint_warm.rs',"""
pub(super) async fn archive<F:Future>(q:Option<(Decimal,Stamp)>,anchor:Decimal,budget:Decimal,step:Decimal,floor:(Decimal,Decimal),keep:Decimal,f:F)->F::Output{
 let data=q.map(|(qty,mark)|Hint{anchor,qty,mark,budget,step,floor,keep});
 READY.scope(Cell::new(data),f).await
}
""")
append('src/core/precheck_hint.rs',"""
#[allow(clippy::too_many_arguments)]
pub async fn archived_entry<C:Curve<Data=Calc>, S:crate::core::costs::Source>(
  core:&Core, old:&DirCtx, cx:&DirCtx, book:&crate::core::types::OrderBook, memo:&Memo<'_,C>, found:&Found<Calc>, anchor:Decimal, src:&S,budget:Decimal,step:Decimal,floor:(Decimal,Decimal)
)->serde_json::Value{
 use super::{precheck_fast::{Want,recalc},size::Fail};
 for (name,n) in [("cpu.threads_max",80),("cpu.reserve_deal",8),("cpu.reserve_calc",24)]{
     core.inner.deps.settings.set(name,crate::settings::value::Scope::Global,Some(serde_json::json!(n)),"archived workload configuration").await.unwrap();
 }
 core.inner.market.write().unwrap_or_else(|p|p.into_inner()).apply(crate::gateway::events::MarketEvent::Book(book.clone()));
 let Some(mark)=stamp(core,old) else{return serde_json::json!({"reason":"missing book stamp"});};
 remember(core,old,mark,memo,found);
 let cached=saved(core,&cx.dir,anchor).map(|c|c.points.len());
 let q=if let Ok(slot)=core.inner.hint_slot.clone().try_acquire_owned(){
   alternative(core,cx,anchor,budget,step,floor,slot).await.map(|(n,m,_)|(n,m))
 }else{None};
 let mut responses=std::collections::BTreeMap::new();
 for n in std::iter::once(anchor).chain(q.map(|x|x.0)) {
   let c=Box::pin(crate::core::costs::evaluate_with(cx,n,src)).await;
   responses.insert(n,c);
 }
 let mut out=Vec::new();
 for mode in [false,true] {
   let mut w=Want::new(anchor);
   let result=super::final_depth::ARCHIVED.scope(responses.clone(),warm::archive(if mode{q}else{None},anchor,budget,step,floor,cx.keep.unwrap(),Box::pin(recalc(core,cx,&mut w,budget,step,floor)))).await;
   out.push(serde_json::json!({"prepared":mode,"path":format!("{:?}",w.path),"selected":result.map(|c|(c.qty,c.profit_usd))}));
 }
 let mut routed=Vec::new();
 for (mode,congested) in [(false,false),(true,false),(true,true)]{
   let _occupied=congested.then(||core.inner.hint_slot.clone().try_acquire_owned().unwrap());
   use futures_util::stream::{FuturesUnordered,StreamExt};
   let (tx,mut rx)=tokio::sync::mpsc::unbounded_channel::<super::final_depth::Request>();
   let mut w=Want::new(anchor);
   let queries=std::cell::Cell::new(0usize);
   let started=std::time::Instant::now();
   let calc=super::final_depth::LIVE_ARCHIVE.scope(tx,warm::archive(if mode{q}else{None},anchor,budget,step,floor,cx.keep.unwrap(),Box::pin(recalc(core,cx,&mut w,budget,step,floor))));
   let serve=async{
     let mut work=FuturesUnordered::new();
     loop{
       tokio::select!{biased;
          Some(())=work.next(),if !work.is_empty()=>{},
          Some((n,optional,reply))=rx.recv()=>{
             queries.set(queries.get()+1);
             work.push(Box::pin(async move{
                let native=crate::gateway::quote_stage::final_scope(Box::pin(crate::core::costs::evaluate_with(cx,n,src)));
                let answer=if optional {crate::gateway::cpu_budget::optional_quote(native).await} else {native.await};
                let _=reply.send(answer);
             }));
          }
       }
     }
   };
   let result=tokio::select!{biased; r=Box::pin(calc)=>r,_=Box::pin(serve)=>unreachable!()};
   routed.push(serde_json::json!({"prepared":mode,"occupied":congested,"path":format!("{:?}",w.path),"selected":result.map(|c|(c.qty,c.profit_usd)),"native_calls":queries.get(),"us":started.elapsed().as_micros()}));
 }
 serde_json::json!({"cached":cached,"native_candidate":found.best.qty,"logged_anchor":anchor,"ready_hint":q.map(|x|x.0),"modes":out,"native_adapter":routed})
}
""".replace('size::Fail','size::Fail').replace('use super::{precheck_fast::{Want,recalc},size::Fail};','use super::precheck_fast::{Want,recalc};'))
for test in ['tests/native_route_release_matrix.rs','tests/core_size_actual_v2.rs']:
 if 'mod report_common;' not in (root/test).read_text():append(test,'\nmod report_common;\n')
rewrite('tests/native_route_release_matrix.rs','let samples:Vec<_>=m.map.values().flatten().map(|p|p.data.clone()).collect();drop(m);','''let samples:Vec<_>=m.map.values().flatten().map(|p|p.data.clone()).collect();
    let floor=(get("threshold"),get("min_pct"));
    let (core,_)=report_common::start(&report_common::dir(&format!("shopnx-{id}-{block}"))).await;
    let entry=if let Some(found)=native.as_ref(){
      let logged=Box::pin(arb::core::precheck_hint::archived_entry(&core,&cx,&cx,&src.book.book,&m,found,get("k_qty"),&src,budget,step,floor)).await;
      let rebuilt=Box::pin(arb::core::precheck_hint::archived_entry(&core,&cx,&cx,&src.book.book,&m,found,found.best.qty,&src,budget,step,floor)).await;
      serde_json::json!({"logged":logged,"rebuilt":rebuilt})
    }else{serde_json::Value::Null};drop(m);''')
rewrite('tests/native_route_release_matrix.rs','"cold":cold,"warm":warm}', '"cold":cold,"warm":warm,"entry":entry}')
rewrite('tests/core_size_actual_v2.rs','let old_points=points.len(); let evals=memo.evals(); drop(memo);','''let (core,_)=report_common::start(&report_common::dir(&format!("source-{}",dir.asset.ticker))).await;
    let logged=Box::pin(arb::core::precheck_hint::archived_entry(&core,&old_ctx,&cx,&a,&memo,&reconstructed,plan.qty,&source,budget,p.step,floor)).await;
    let rebuilt=Box::pin(arb::core::precheck_hint::archived_entry(&core,&old_ctx,&cx,&a,&memo,&reconstructed,reconstructed.best.qty,&source,budget,p.step,floor)).await;
    let entry=serde_json::json!({"logged":logged,"rebuilt":rebuilt});
    let old_points=points.len(); let evals=memo.evals(); drop(memo);''')
rewrite('tests/core_size_actual_v2.rs','"cold":cold,"warm":warm}', '"cold":cold,"warm":warm,"entry":entry}')
