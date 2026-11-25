from pathlib import Path
import sys
root=Path(sys.argv[1])
p=root/'src/core/size_search.rs'; s=p.read_text()
def inject(mark, diagnostic):
 global s
 assert s.count(mark)==1,mark
 s=s.replace(mark,diagnostic+'\n'+mark)
def emit(phase, items):
 return '    println!("TRACE0392 {}", serde_json::json!({"stage":"'+phase+'","cap":p.max_evals,'+items+'}));'
inject('    m.arm(p.ext.deadline);',emit('bounds','"interval":[lo,hi],"known":m.map.keys().copied().collect::<Vec<_>>()'))
inject('    if !spans.is_empty() {',emit('grid','"points":ok,"refine_spans":spans,"tolerance_steps":tol,"early":early'))
inject('    let capped = m.capped();',emit('refined','"seen":m.map.keys().copied().collect::<Vec<_>>()'))
inject('    let choice = SizeChoice {',emit('after_pct','"seen":m.map.keys().copied().collect::<Vec<_>>(),"moved":pct_moved'))
p.write_text(s)
p=root/'tests/size_matrix_routes.inc';s=p.read_text()
a='            // All fees are nonnegative.'
assert s.count(a)==1;s=s.replace(a,'            if s.label!="dex-split-cex" || trial!=392 {continue;}\n'+a)
a='                let start = Instant::now();'; assert s.count(a)==1
b='''                let oracle_rows: Vec<_> = (1..=steps).filter_map(|n| {
                        let q=s.step*Decimal::from(n);
                        let x=s.point(q).ok()?;
                        let buying=s.buy.price(q,true,s.budget)?;
                        let selling=s.sell.price(x.arrive,false,s.budget)?;
                        Some(json!([q,x.profit,x.arrive,x.data.0,buying.0,buying.2,selling.0]))
                    }).collect();
'''
s=s.replace(a,b+a)
a='                let elapsed = start.elapsed().as_micros();'; assert s.count(a)==1
b='''                let observed: Vec<_> = replay.evaluated.iter().map(|(q,r)|json!([q,r.as_ref().ok().map(|v|v.profit)])).collect();
                println!("TRACE0392 {}",json!({"stage":"selected","cap":max_evals, "seed":seed, "step":s.step, "guess":guess,
                    "withdraw":s.withdraw, "arrive_keep":s.arrive_keep, "floor":floor, "budget":s.budget,
                    "buy_breaks":s.buy.breaks(),"sell_breaks":s.sell.breaks(),"observed":observed,"oracle_rows":oracle_rows,
                    "found":actual.as_ref().ok().map(|a|a.best.qty),"oracle":best.as_ref().map(|a|a.qty)}));
'''
p.write_text(s.replace(a,a+'\n'+b))

p=root/'tests/core_size_release_matrix.rs'
s=p.read_text()
assert s.count('            arrive: arrived,')==1
p.write_text(s.replace('            arrive: arrived,','            arrive: (qty-self.withdraw)*self.arrive_keep,'))
p=root/'tests/size_matrix_routes.inc'
s=p.read_text()
s=s.replace('let selling=s.sell.price(x.arrive,false,s.budget)?;', 'let sold=(x.arrive/s.step).floor()*s.step; let selling=s.sell.price(sold,false,s.budget)?;').replace('x.profit,x.arrive,x.data.0','x.profit,sold,x.data.0')
a='                let p = native_params(&s, guess, floor, max_evals);'
assert s.count(a)==1
b='''                let mut p = native_params(&s, guess, floor, max_evals);
                let lane=std::env::var("TRACE0392_LANE").unwrap();
                if lane=="regions" {
                    let Market::Ladder(book)=&s.sell else {panic!("expected historical fixture ladder");};
                    let mut before=Decimal::ZERO;
                    let levels:Vec<_>=book.0.iter().map(|b|{
                        let qty=b.end_qty-before; before=b.end_qty;
                        arb::core::types::PriceLevel {price:b.price,qty}
                    }).collect();
                    let pair=arb::gateway::events::SpotPairState{
                        ticker:"fixture".into(),status:1,online:true,api_buy:true,api_sell:true,
                        step_size:s.step,min_notional:Some(s.min_sale),max_notional:Some(s.max_sale),
                        min_qty:None,max_qty:None,order_block:None,price_band:None};
                    p.ext.sell_regions=arb::core::size_scan::regions(&levels,Some(&pair));
                    p.ext.no_formula=true;
                    p.ext.budget_monotone=false;
                }'''
p.write_text(s.replace(a,b).replace('"stage":"selected","cap":max_evals','"stage":"selected","lane":lane,"sell_regions":p.ext.sell_regions,"cap":max_evals'))
