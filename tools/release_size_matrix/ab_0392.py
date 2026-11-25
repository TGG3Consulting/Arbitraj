from pathlib import Path
import sys
src=Path(__file__).parent; root=Path(sys.argv[1]); f=root/'tests/core_size_release_matrix.rs';s=f.read_text()
assert s.count('            arrive: arrived,')==1
f.write_text(s.replace('            arrive: arrived,','            arrive: (qty-self.withdraw)*self.arrive_keep,'))
f=root/'tests/size_matrix_routes.inc';s=f.read_text().replace('for max_evals in CAPS {','for max_evals in [300u32] {')
a='                let p = native_params(&s, guess, floor, max_evals);';assert s.count(a)==1
b='''                let mut p=native_params(&s,guess,floor,max_evals);
                p.ext.no_formula=matches!(s.buy,Market::Router(_)) || matches!(s.sell,Market::Router(_));
                p.ext.budget_monotone=matches!(s.buy,Market::Ladder(_));
                if let Market::Ladder(book)=&s.sell {
                    let mut before=Decimal::ZERO;
                    let levels:Vec<_>=book.0.iter().map(|b|{
                        let qty=b.end_qty-before;before=b.end_qty;arb::core::types::PriceLevel{price:b.price,qty}
                    }).collect();
                    let pair=arb::gateway::events::SpotPairState {
                        ticker:"fixture".into(),status:1,online:true,api_buy:true,api_sell:true,
                        step_size:s.step,min_notional:Some(s.min_sale),max_notional:Some(s.max_sale),
                        min_qty:None,max_qty:None,order_block:None,price_band:None};
                    p.ext.sell_regions=arb::core::size_scan::regions(&levels,Some(&pair));
                }'''
s=s.replace(a,b)
a='                let pnl = |p: Option<&Sample>|'
assert s.count(a)==1
b='''                let extra_start=Instant::now();
                let (b,priced,probed)=plateau_trial(&s,floor,&replay.evaluated,found.map(|x|(x.choice.min_qty,x.choice.max_qty,x.best.qty)),pass,max_evals);
                let extra_ns=extra_start.elapsed().as_nanos() as u64;
                let metric=|a:Option<&Sample>|a.map(|p|[p.profit,p.invested,p.qty]);
                assert!(metric(b.as_ref()).is_none_or(|_|best.as_ref().is_some_and(|upper|rank(upper)>=rank(b.as_ref().unwrap()))));
                assert!(pass.is_none_or(|old|b.as_ref().is_some_and(|new|rank(new)>=rank(old))));
                assert!(priced<=24 && (priced==0 || replay.evaluated.len()+priced<=300));
                println!("PLATEAU_CONTROL {}",json!({"scenario":format!("routes-{}-{trial:04}", s.label),"seed":seed,
                    "a":metric(pass),"b":metric(b.as_ref()),"oracle":metric(best.as_ref()),"evaluated":replay.evaluated.len(),"extra":priced,
                    "plateau_candidates":probed,"native_us":elapsed,"extra_ns":extra_ns,
                    "eligible":matches!(s.buy,Market::Router(_))&&matches!(s.sell,Market::Ladder(_))&&s.arrive_keep<Decimal::ONE}));
'''
s=s.replace(a,b+a)
f.write_text(s+'\n'+(src/'plateau_trial.rs').read_text(encoding='utf-8-sig'))
