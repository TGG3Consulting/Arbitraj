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
p=root/'tests/core_size_actual_v2.rs';s=p.read_text()
a='let (pick,_,rerated)=rank();'
assert s.count(a)==1
s=s.replace(a,a+'''
    let production=arb::core::precheck_hint::evidenced_hint(&points,&cx,budget,floor,None);
    assert_eq!(production, Some(pick), "release and independent replay must match");
''');p.write_text(s)
p=root/'tests/native_route_release_matrix.rs';s=p.read_text()
s=s.replace('let full_started=std::time::Instant::now(); let selected=', '''let full_started=std::time::Instant::now();
        if rev {
            use arb::core::{size_memo::Memo,size_search::search_in};
            let mut m=Memo::new(&mut curve,step,budget,300).with_share(p.par_points,p.reuse).with_gate(p.ext.min_usd,p.ext.min_pct);
            let native=search_in(&mut m,&p).await.unwrap();
            let samples:Vec<_>=m.map.values().flatten().map(|p|p.data.clone()).collect();drop(m);
            let floor=(get("threshold"),get("min_pct"));
            let t=std::time::Instant::now(); let alt=arb::core::precheck_hint::evidenced_hint(&samples,&cx,budget,floor,Some((&src.book.book.bids,num("0.001"),step)));
            let cpu=t.elapsed().as_micros();
            let old=curve.at(get("k_qty")).await.ok();
            let checked=match alt {Some(q)=>curve.at(q).await.ok().filter(|p|p.invested<=budget && p.profit>=floor.0 && p.data.profit_pct>=floor.1),None=>None};
            let rank=|p:&arb::core::size::Point<arb::core::costs::Calc>|(p.profit,std::cmp::Reverse((p.invested,p.qty)));
            let chosen=old.iter().chain(checked.iter()).max_by_key(|p|rank(p));
            println!("NATIVE_NEIGHBOUR {}",json!({"id":id,"block":block,"points":samples.len(),"initial":old.map(|p|[p.qty,p.profit]),"alternate":checked.map(|p|[p.qty,p.profit]),"chosen":chosen.map(|p|[p.qty,p.profit]),"hint_us":cpu,"native_best":native.best.profit}));
        }
        let selected=''').replace('"initial":old.map','"initial":old.as_ref().map').replace('"alternate":checked.map','"alternate":checked.as_ref().map')
assert 'NATIVE_NEIGHBOUR' in s;p.write_text(s)
