//! Economic checks on the recorded size only. The archive cannot quote different sizes.
mod size_common;
use arb::core::{card_max,convert::{self,Rates},reserve,types::{Cost,CostKind}};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json,Value};
use std::{collections::HashMap,fs};

#[derive(Deserialize)]
struct Line {
    amount: Decimal, asset: String, kind: CostKind, stage: String,
    usdt: Option<Decimal>, in_price: bool, in_qty: bool,
}
#[derive(Deserialize)]
struct Point {
    coin: String, quotes: Vec<String>, costs: Vec<Line>, qa:[Decimal;3], qb:[Decimal;3],
    qty:Decimal, sell_qty:Decimal, applied_sell:Decimal, k:Decimal, sigma:Decimal, transit:Decimal,
    closure:Option<Decimal>,floor:Decimal,pct:Decimal,orig_profit:Decimal,
}
fn calculate(p:Point, gate:&mut arb::core::costs::Calc)->Result<Value,String>{
    if p.qty!=p.qa[0] || p.sell_qty!=p.qb[0] || p.qa[1]<p.qty || p.qb[1]<p.sell_qty {
        return Err("archived quote and required quantity differ".into());
    }
    if p.qty<=Decimal::ZERO||p.sell_qty<=Decimal::ZERO||p.qa[2]<=Decimal::ZERO||p.qb[2]<=Decimal::ZERO {
        return Err("non-positive archived quantity or price".into());
    }
    let prices:HashMap<_,_>=p.costs.iter().filter_map(|l| l.usdt.and_then(|u|u.checked_div(l.amount)).map(|r|(l.asset.clone(),r))).collect();
    let rates=Rates{quote:p.quotes,coin:p.coin.clone(),coin_price:p.qa[2],prices};
    let invested=p.qty*p.qa[2];
    let quoted_proceeds=p.sell_qty*p.qb[2];
    let keep=reserve::keep_of(p.k,p.sigma,p.transit).ok_or("missing transit")?;
    let risk=reserve::amount(keep,quoted_proceeds,p.closure,p.qa[2],p.qb[2]);
    let mut outgoing=Decimal::ZERO;
    let mut reserves=0;
    for l in &p.costs {
        if l.kind==CostKind::PriceMoveReserve {reserves+=1;continue}
        let cost=Cost::new(l.kind,l.amount,l.asset.clone(),l.in_price);
        let in_quantity=(convert::in_coin(&cost,&p.coin)&&l.stage!="B")||cost.kind==CostKind::StepRemainder;
        if in_quantity!=l.in_qty{return Err("archived fee classification differs".into());}
        if !cost.in_price {
            let value=convert::to_usdt(&cost,&rates).map_err(|_|"nonzero or unpriced cost cannot be converted")?;
            if !in_quantity {outgoing+=value;}
        }
    }
    if reserves!=1{return Err("expected one historical reserve".into());}
    let quoted=quoted_proceeds-invested-outgoing-risk;
    let after=quoted+p.sell_qty*(p.applied_sell-p.qb[2]);
    let mut passes=|profit:Decimal| {
        gate.profit_usd=profit;
        gate.profit_pct=profit/invested*Decimal::ONE_HUNDRED;
        card_max::passes(gate,p.floor,p.pct)
    };
    Ok(json!({"quoted":quoted,"after":after,"quoted_pass":passes(quoted),"pass":passes(after),
              "historical_plan":p.orig_profit,"changed_sale_price":p.applied_sell!=p.qb[2]}))
}
#[tokio::test]
async fn recorded_points(){
    let fixture=size_common::Rec::load("archive-recorded-points").await;
    // Only the two profit fields are read by the native threshold check.
    let mut gate=size_common::calc::calc(&fixture,Decimal::TEN);
    let value:Value=serde_json::from_slice(&fs::read(std::env::var("ARCHIVE_IN").unwrap()).unwrap()).unwrap();
    let mut rows=vec![];
    for v in value["cases"].as_array().unwrap() {
        if v["cycle"]==true{
            rows.push(cycle_check(v));
        }else{
            let p:Point=serde_json::from_value(v.clone()).expect("archived point");
            rows.push(calculate(p,&mut gate).unwrap_or_else(|e|json!({"unavailable":e})));
        }
    }
    assert_eq!(rows.len(),12766);
    fs::write(std::env::var("ARCHIVE_OUT").unwrap(),serde_json::to_vec(&rows).unwrap()).unwrap();
}
fn cycle_check(v:&Value)->Value {
    let dec=|key:&str| v[key].as_str().unwrap().parse::<Decimal>().unwrap();
    let input=dec("raw_in");
    let output=dec("raw_out");
    let net_base=output-input-dec("cost_base");
    let profit=net_base*dec("base_usdt");
    json!({"cycle":true,"after":profit,"quoted":profit,"pass":profit>=dec("floor"),"quoted_pass":profit>=dec("floor")})
}
