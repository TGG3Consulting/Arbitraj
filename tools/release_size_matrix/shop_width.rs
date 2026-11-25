use arb::core::size::{Params, Curve};
use arb::core::size_curve::SourceCurve;
use arb::core::size_search::search;
use std::collections::BTreeSet;
use rust_decimal::prelude::ToPrimitive;

#[tokio::test]
async fn shop_width_and_size() {
    let raw:Value=serde_json::from_str(include_str!("control_shop_book.json")).unwrap();
    let symbols:Value=serde_json::from_str(include_str!("control_shop_symbol.json")).unwrap();
    let rules=&symbols["data"]["symbols"][0];let observed=&raw["data"][0];
    let budget=num("350");let step=num("0.000001");let fee=num("0.001");
    for width in [2i16,3,4,5,12] {
        let parts=shopinx::scene_words(width).await;
        configure(&parts,"graph").await;
        shopinx::partial(&parts,Some(true)).await;
        for rev in [false,true] {
            let ds=if rev {Side::Buy} else {Side::Sell};
            let bs=if rev {Side::Sell} else {Side::Buy};
            let rq=shopinx::full(&parts.quoting(),ds,num("57000")).await.unwrap();
            let cap=shopinx::full(&parts.quoting(),ds,num("100000")).await.unwrap();
            println!("SHOP_WIDTH {}",json!({"type":"dex","width":width,"side":format!("{ds:?}"),"q57000":rq.fillable_qty,"value57000":rq.avg_price*rq.fillable_qty,"q100000":cap.fillable_qty}));
            for (name,h,enforce) in [("top",num("0.7"),true),("top",num("0.7"),false),("control",Decimal::ONE,false),("control",num("0.7"),false),("control",num("0.7"),true)] {
                if ![2,12].contains(&width) && (name!="control" || h!=num("0.7") || enforce) {continue;}
                let (price,depth)=if name=="top" {
                   let (px,qty)=if rev {("bidPrice","bidVolume")} else {("askPrice","askVolume")};
                   (num(observed[px].as_str().unwrap()),num(observed[qty].as_str().unwrap()))
                } else {(if rev {num("0.012")} else {num("0.006")},num("100000"))};
                let dir=Direction{asset:shopinx::shopinx(),buy_on:VenueId(if rev {"bsc_dex"} else {"bingx"}.into()),sell_on:VenueId(if rev {"bingx"} else {"bsc_dex"}.into())};
                let mut cx=ctx(&dir,h);let label=if rev {"dex_bingx"} else {"bingx_dex"};
                let rule=serde_json::from_value(json!({"ticker":"SHOPINX","online":true,"status":1,"api_buy":true,"api_sell":true,
                   "min_qty":if enforce {rules["minQty"].clone()} else {Value::Null},"max_qty":if enforce {rules["maxQty"].clone()} else {Value::Null},
                   "min_notional":rules["minNotional"],"max_notional":rules["maxNotional"],"step_size":rules["stepSize"]})).unwrap();
                if rev {cx.pair_b=Some(rule)} else {cx.pair_a=Some(rule)};
                let level=PriceLevel{price,qty:depth};let now=Utc::now();let venue=if rev {dir.sell_on.clone()}else{dir.buy_on.clone()};
                let book=OrderBook{venue:venue.clone(),asset:dir.asset.clone(),received_at:now,exchange_time:None,
                     asks:if rev {vec![]}else{vec![level.clone()]},bids:if rev {vec![level]}else{vec![]}};
                let mut ext=arb::core::size_extra::Extra::default();ext.min_usd=Some(num("0.05"));ext.min_pct=Some(Decimal::ZERO);ext.no_formula=true;ext.budget_monotone=!rev;
                if rev {ext.sell_regions=arb::core::size_scan::regions(&book.bids,cx.pair_b.as_ref())}
                let source=TwoLeg{parts:&parts,reverse:rev,book:FrozenBook{venue,side:bs,asset:dir.asset.clone(),book,fee:BookFee::in_received(fee,bs,"SHOPINX","USDT"),taken_at:now}};
                let p=Params{step,budget,guess:budget/price,buy_breaks:if rev{vec![]}else{vec![depth]},sell_breaks:if rev{vec![depth]}else{vec![]},bisect_steps:40,grid_points:16,refine_steps:30,tolerance_pct:Decimal::ONE,max_evals:300,par_points:1,bisect_probes:3,early_floor:None,reuse:true,par_blocked:false,ext};
                let mut curve=SourceCurve{cx:&cx,src:&source};
                let native=search(&mut curve,&p).await;
                let selected=native.as_ref().ok().map(|f|json!({"q":f.best.qty,"spend":f.best.data.spend_max,"sell":f.best.data.sell_qty,"net":f.best.profit,"evals":f.choice.evals,"passed":passes(&f.best.data,num("0.05"),Decimal::ZERO),"hi":f.choice.max_qty}));
                // Distinct from native search: dense 500-token grid, plus boundaries derived from two legs, then 10-token
                // and 0.25-token scans around the winner. This lower bound must not be called an exact micro-step optimum.
                let bound=if rev {cap.fillable_qty.min(depth*h)} else {((cap.fillable_qty*h+Decimal::ONE)/(Decimal::ONE-fee)).min(depth).min(budget/price)};
                let end=(bound/num("500")).floor().to_u64().unwrap_or(0).min(220);
                let mut points:BTreeSet<_>=(1..=end).map(|i| Decimal::from(i*500)).collect();
                let limit=(bound/step).floor()*step;
                for offset in [Decimal::ZERO,step,num("0.01"),Decimal::ONE] {if limit>offset {points.insert(limit-offset);}}
                points.insert(num("57505"));points.insert(num("100"));points.insert(num("250"));
                let mut best:Option<(Decimal,Decimal,Decimal)>=None;
                let mut tested=0;
                for round in 0..3 {
                    for q in std::mem::take(&mut points) {
                        if q<=Decimal::ZERO {continue;}
                        tested+=1;
                        if let Ok(pt)=curve.at(q).await && pt.invested<=budget && passes(&pt.data,num("0.05"),Decimal::ZERO) {
                            let rank=(pt.profit,-pt.invested,-q);
                            if best.is_none_or(|b|rank>b) {best=Some(rank);}
                        }
                    }
                    if let Some((_,_,negq))=best && round<2 {
                       let (d,n)=if round==0 {(num("10"),50)}else{(num("0.25"),40)};
                       for i in -n..=n {points.insert(-negq+Decimal::from(i)*d);}
                    }
                }
                println!("SHOP_WIDTH {}",json!({"type":"both_legs","width":width,"direction":label,"book":name,"h":h,"min_qty":enforce,"q_boundary":limit,
                   "native":selected,"native_error":native.as_ref().err().map(|e|format!("{e:?}")),"challenger":best.map(|(p,s,q)|json!({"q":-q,"spend":-s,"net":p})),"challenger_evals":tested,
                   "price":price,"depth":depth,"budget":budget,"step":step}));
            }
        }
    }
}
