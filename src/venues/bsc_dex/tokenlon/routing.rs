//! Штатный выбор Tokenlon. Чистая стоимость сравнивается с полным имеющимся маршрутом на том же количестве.
use super::{client,profile,receipt::Proof,sim,wire};
use crate::{
    core::types::{Asset,Cost,CostKind,DexRoute,Quote,RouteMode,Side},
    gateway::{errors::{ErrorCode,GatewayResult},quote_stage},
    venues::bsc_dex::{exec,gas_ticker,hashflow,num,parts::Parts,quote::direct,send,wallet},
};
use alloy::signers::{SignerSync,local::PrivateKeySigner};
use alloy_primitives::{Address,Bytes,U256};
use chrono::Utc;
use rust_decimal::Decimal;
use std::time::Duration;

pub(in crate::venues::bsc_dex) struct Ticket {
    pub(super) proof:Proof,pub(super) data:Bytes,pub(super) qty:Decimal,pub(super) side:Side,
    pub(super) decs:(u8,u8),pub(super) revision:u64,
}

fn market(p:&Parts,asset:&Asset)->Option<(Address,Address)>{
    let base=asset.contract?;
    let quote=p.tokens.usdt(&p.ctx).ok()?;
    (base!=quote && (base==wire::WETH)==exec::native::is_native(&p.ctx,asset)).then_some((base,quote))
}
pub(in crate::venues::bsc_dex) fn available(p:&Parts,asset:&Asset,side:Side)->bool{
    super::enabled(&p.ctx) && market(p,asset).is_some_and(|(b,q)|!p.tokenlon.rates(p,b,q,side).is_empty())
}
pub(in crate::venues::bsc_dex) fn breaks(p:&Parts,asset:&Asset,side:Side,max:Decimal)->Vec<Decimal>{
    if !super::enabled(&p.ctx){return vec![];}
    market(p,asset).map(|(b,q)|p.tokenlon.rates(p,b,q,side).into_iter().filter_map(|(n,_)|(n<=max).then_some(n)).collect()).unwrap_or_default()
}

fn signed(raw:&serde_json::Value,key:&PrivateKeySigner,i:Address,o:Address)->Option<(Proof,Bytes)> {
    let mut salt=[0u8;32];getrandom::fill(&mut salt).ok()?;
    let terms=wire::Terms {payer:key.address(),recipient:key.address(),input:i,output:o,
        spent:super::quantity(&raw["takerAssetAmount"])?,minimum:U256::from(1)};
    let now=u64::try_from(Utc::now().timestamp()).ok()?;
    let prepared=wire::prepare(raw,&terms,U256::from_be_bytes(salt),now)?;
    let sig=key.sign_hash_sync(&prepared.sign_digest).ok()?;
    let data=prepared.calldata(&sig)?;
    Some((prepared.proof,data))
}

pub(in crate::venues::bsc_dex) async fn consider(p:&Parts,asset:&Asset,side:Side,qty:Decimal,old:GatewayResult<Quote>)->GatewayResult<Quote>{
    if !super::enabled(&p.ctx) || qty<=Decimal::ZERO || quote_stage::settlement()
        || old.as_ref().is_err_and(|e|e.code!=ErrorCode::NoRoute){return old;}
    let cutoff=if let Some(v)=old.as_ref().ok().filter(|q|q.qty==qty && q.fillable_qty==qty &&
        (!quote_stage::is_final() || q.route.as_ref().is_none_or(|r|r.executable))){
        let Some(net)=hashflow::routing::total(p,v).await else{return old};
        Some(net)
    } else {None};
    let Ok(ms)=p.ctx.int("tokenlon.quote_budget_ms") else{return old};
    tokio::time::timeout(Duration::from_millis(ms),candidate(p,asset,side,qty,cutoff)).await.ok().flatten().map_or(old,Ok)
}
async fn candidate(p:&Parts,asset:&Asset,side:Side,qty:Decimal,cutoff:Option<Decimal>)->Option<Quote>{
    let c=&p.ctx;
    let rev=c.settings.generation();
    let (base,quote)=market(p,asset)?;
    let depth=p.tokenlon.rates(p,base,quote,side);
    if depth.is_empty(){return None;}
    let buy=side==Side::Buy;
    let taxes=direct::taxes(&p.quoting(),asset).ok()?;
    let _=taxes;
    let head=p.heads.fresh(c).ok()?;
    p.lag.check(c).ok()?;
    let cap=c.dec("tokenlon.gas_budget_native").ok()?;
    if cap<=Decimal::ZERO || send::tip(&send::select(c,&p.stats).ok()?).is_some(){return None;}
    let ticker=gas_ticker::read(c).ok()?;
    let comparison=|net:Decimal|cutoff.is_none_or(|x|if buy {net<x} else {net>x});
    // Лестница SDK хранит только измеренные размеры; окончательный запрос допускает и новый размер.
    let observed=depth.into_iter().find_map(|(n,rate)|(n==qty).then_some(rate));
    if !quote_stage::is_final() && observed.is_none(){return None;}
    let mut q=Quote {venue:c.venue(),asset:asset.clone(),side,qty,fillable_qty:qty,
        avg_price:observed.and_then(|r|if buy {Decimal::ONE.checked_div(r)}else{Some(r)}).unwrap_or(Decimal::ZERO),
        costs:vec![Cost::new(CostKind::Gas,cap,ticker.clone(),false)],signal_route:None,at:Utc::now(),pick:Some(super::LABEL.into()),
        route:Some(DexRoute {mode:RouteMode::RfqIndicative,legs:vec![],block:head.number,gas_units:0,executable:false,exec_note:Some(super::LABEL.into())})};
    if !quote_stage::is_final(){return comparison(hashflow::routing::total(p,&q).await?).then_some(q);}
    c.bound().ok()?;
    let key=wallet::signer::signer(c).ok()?;
    let m=profile::markets(c)?.into_iter().find(|m|m.base_token==base && m.quote_token==quote)?;
    let (i,o)=if buy {(quote,base)}else{(base,quote)};
    let (db,dc)=tokio::try_join!(p.tokens.decimals(c,base),p.tokens.decimals(c,quote)).ok()?;
    let target=direct::ceil_units(qty,db)?;
    if num::to_decimal(target,db)?!=qty{return None;}
    let mut req=client::Query{base:m.base,quote:m.quote,side,amount:qty,base_dec:db,input:i,output:o};
    let issued=client::firm(c,&key,&req).await.ok()?;
    let (mut proof,mut data)=signed(&issued.raw_order,&key,i,o)?;
    if buy && proof.net<target {
        let denominator=10000u16.checked_sub(proof.fee)?;
        let next=target.checked_mul(U256::from(10000))?.div_ceil(U256::from(denominator));
        req.amount=num::to_decimal(next,db)?;
        if num::to_units(req.amount,db)?!=next{return None;}
        let fresh=client::firm(c,&key,&req).await.ok()?;
        (proof,data)=signed(&fresh.raw_order,&key,i,o)?;
    }
    if (buy && proof.net<target) || (!buy && proof.terms.spent!=target) || !super::timely(c,proof.expiry){return None;}
    let (block,gas)=sim::run(c,&proof,&data).await.ok()?;
    let mut pricing=p.quoting();
    pricing.gas=pricing.gas.for_quote(c,&p.heads).await.ok()?;
    let unit=exec::cycle_gas::priced(&pricing,key.address(),(wire::USER_PROXY,data.clone())).await.ok()?;
    let bill=exec::cycle_gas::simulated(&pricing,unit,gas,0,0).ok()?;
    let cost=num::to_decimal(bill.wei(),18)?;
    if cost>cap{return None;}
    let cash=if buy {num::to_decimal(proof.terms.spent,dc)?}else{num::to_decimal(proof.net,dc)?};
    q.avg_price=cash.checked_div(qty)?;
    q.costs=vec![Cost::new(CostKind::Gas,cost,ticker,false)];
    q.route=Some(DexRoute{mode:RouteMode::Rfq(proof.tx_hash),legs:vec![],block,gas_units:gas,executable:true,exec_note:Some(super::LABEL.into())});
    if !comparison(hashflow::routing::total(p,&q).await?) || rev!=c.settings.generation() || !super::timely(c,proof.expiry) {return None;}
    p.tokenlon.save(Ticket{proof,data,qty,side,decs:(db,dc),revision:rev});
    Some(q)
}
