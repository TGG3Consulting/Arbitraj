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
