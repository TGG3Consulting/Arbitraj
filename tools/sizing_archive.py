from pathlib import Path
import sys
root=Path(sys.argv[1])
p=root/'src/core/precheck_hint.rs'
with p.open('a') as out:
 out.write("""
pub fn evidenced_hint(points:&[Calc], cx:&DirCtx, budget:Decimal, floor:(Decimal,Decimal),
    book:Option<(&[crate::core::types::PriceLevel], Decimal, Decimal)>)->Option<Decimal>{
    let mut samples:Vec<_>=points.iter().filter_map(Sample::from_calc).collect();
    samples.sort_unstable_by_key(|s|s.qty);
    samples.dedup_by_key(|s|s.qty);
    let model=Model{keep:cx.keep?,share:cx.closure_share,h:cx.h,budget,step:book.map_or(Decimal::ONE, |x|x.2),floor};
    match book{None=>risk_best(&samples,&model),Some((bids,fee,_))=>v3::best(&samples,bids,&model,fee)}
}
""")
p=root/'src/core/precheck_hint.rs'
with p.open('a') as out:
 out.write("""
pub async fn archive_compare<S:crate::core::costs::Source>(cx:&DirCtx, src:&S, budget:Decimal, floor:(Decimal,Decimal), original:Decimal, alt:Option<Decimal>, warm:bool)->(Option<(Decimal,Decimal)>,bool,u128,usize) {
    use std::sync::atomic::{AtomicUsize,Ordering};
    let calls=AtomicUsize::new(0);
    let calls=&calls;
    let fresh=|q:Option<Decimal>|async move{
        let q=q.ok_or_else(String::new)?;
        calls.fetch_add(1,Ordering::Relaxed);
        let c=Box::pin(crate::core::costs::evaluate_with(cx,q,src)).await.map_err(|e|e.to_string())?;
        if c.spend_max>budget{return Err(String::from("budget"));}
        crate::core::precheck_fast::accept(c,q,budget,&cx.dir.asset.ticker,floor).map(|c|(c,()))
    };
    let t=std::time::Instant::now();
    let (r,moved)=crate::core::precheck_fast::compare_checked(Box::pin(fresh(Some(original))),Box::pin(fresh(alt.filter(|q|q!=&original))),warm,|_| true).await;
    (r.ok().map(|(c,_)|(c.qty,c.profit_usd)),moved,t.elapsed().as_micros(),calls.load(Ordering::Relaxed))
}
""")
p=root/'tests/core_size_actual_v2.rs';s=p.read_text()
a='let (pick,_,rerated)=rank();'
assert s.count(a)==1
s=s.replace(a,a+'''
    let production=arb::core::precheck_hint::evidenced_hint(&points,&cx,budget,floor,None);
    let cold=arb::core::precheck_hint::archive_compare(&cx,&source,budget,floor,plan.qty,production,false).await;
    let warm=arb::core::precheck_hint::archive_compare(&cx,&source,budget,floor,plan.qty,production,true).await;
    println!("NATIVE_RISK_PRECHECK {}",serde_json::json!({"original":plan.qty,"choice":production,"cold":cold,"warm":warm}));
    assert_eq!(production, Some(pick), "release and independent replay must match");
''');p.write_text(s)
p=root/'tests/native_route_release_matrix.rs';s=p.read_text()
s=s.replace('let full_started=std::time::Instant::now(); let selected=', '''let full_started=std::time::Instant::now();
        if rev {
            use arb::core::{size_memo::Memo,size_search::search_in};
            let mut m=Memo::new(&mut curve,step,budget,300).with_share(p.par_points,p.reuse).with_gate(p.ext.min_usd,p.ext.min_pct);
            let native=search_in(&mut m,&p).await.ok();
            let samples:Vec<_>=m.map.values().flatten().map(|p|p.data.clone()).collect();drop(m);
            let floor=(get("threshold"),get("min_pct"));
            let t=std::time::Instant::now(); let alt=arb::core::precheck_hint::evidenced_hint(&samples,&cx,budget,floor,Some((&src.book.book.bids,num("0.001"),step)));
            let cpu=t.elapsed().as_micros();
            let cold=arb::core::precheck_hint::archive_compare(&cx,&src,budget,floor,get("k_qty"),alt,false).await;
            let warm=arb::core::precheck_hint::archive_compare(&cx,&src,budget,floor,get("k_qty"),alt,true).await;
            let old=curve.at(get("k_qty")).await.ok();
            let checked=match alt {Some(q)=>curve.at(q).await.ok().filter(|p|p.invested<=budget && p.profit>=floor.0 && p.data.profit_pct>=floor.1),None=>None};
            let rank=|p:&arb::core::size::Point<arb::core::costs::Calc>|(p.profit,std::cmp::Reverse((p.invested,p.qty)));
            let chosen=old.iter().chain(checked.iter()).max_by_key(|p|rank(p));
            println!("NATIVE_NEIGHBOUR {}",json!({"id":id,"block":block,"points":samples.len(),"initial":old.map(|p|[p.qty,p.profit]),"alternate":checked.map(|p|[p.qty,p.profit]),"chosen":chosen.map(|p|[p.qty,p.profit]),"hint_us":cpu,"native_best":native.map(|v|v.best.profit),"cold":cold,"warm":warm}));
        }
        let selected=''').replace('"initial":old.map','"initial":old.as_ref().map').replace('"alternate":checked.map','"alternate":checked.as_ref().map')
assert 'NATIVE_NEIGHBOUR' in s;p.write_text(s)
