//! Штатный выбор Tokenlon. Чистая стоимость сравнивается с полным имеющимся маршрутом на том же количестве.
use super::{client,profile,receipt::Proof,sim,wire};
use crate::{
    core::types::{Asset,Cost,CostKind,DexRoute,Quote,RouteMode,Side},
    gateway::quote_stage,
    venues::bsc_dex::{exec,gas_ticker,num,parts::Parts,quote::direct,send,wallet},
};
use alloy::signers::{SignerSync,local::PrivateKeySigner};
use alloy_primitives::{Address,Bytes,U256};
use chrono::Utc;
use rust_decimal::Decimal;

pub(in crate::venues::bsc_dex) struct Ticket {
    pub(super) proof:Proof,pub(super) data:Bytes,pub(super) qty:Decimal,pub(super) side:Side,
    pub(super) decs:(u8,u8),pub(super) revision:u64,
}

fn market(p:&Parts,asset:&Asset)->Option<(Address,Address)>{
    let base=asset.contract?;
    let weth=p.ctx.addr("dex.native_wrapped").ok()?;
    let quote=p.tokens.usdt(&p.ctx).ok()?;
    (base!=quote && (base==weth)==exec::native::is_native(&p.ctx,asset)).then_some((base,quote))
}
pub(in crate::venues::bsc_dex) fn available(p:&Parts,asset:&Asset,side:Side)->bool{
    super::enabled(&p.ctx) && market(p,asset).is_some_and(|(b,q)|!p.tokenlon.rates(p,b,q,side).is_empty())
}
pub(in crate::venues::bsc_dex) fn breaks(p:&Parts,asset:&Asset,side:Side,max:Decimal)->Vec<Decimal>{
    if !super::enabled(&p.ctx){return vec![];}
    let Some((b,q))=market(p,asset) else{return vec![]};
    let Some(dec)=p.tokens.decimals_known(b) else{return vec![]};
    let mut sizes=std::collections::BTreeSet::new();
    for (size,rate) in p.tokenlon.rates(p,b,q,side) {
        let low=rate.min.round_dp_with_strategy(u32::from(dec),rust_decimal::RoundingStrategy::ToPositiveInfinity);
        let high=max.min(rate.max).round_dp_with_strategy(u32::from(dec),rust_decimal::RoundingStrategy::ToZero);
        sizes.extend([size,low,high].into_iter().filter(|v|*v>Decimal::ZERO && *v>=low && *v<=high));
    }
    sizes.into_iter().collect()
}

fn prepare(c:&super::super::ctx::Ctx,raw:&serde_json::Value,key:&PrivateKeySigner,i:Address,o:Address)->Option<wire::Prepared> {
    let mut salt=[0u8;32];getrandom::fill(&mut salt).ok()?;
    let roles=super::roles::FrozenRoles::read(c)?;
    let terms=wire::Terms {roles,payer:key.address(),recipient:key.address(),input:i,output:o,
        spent:super::quantity(&raw["takerAssetAmount"])?,minimum:U256::from(1)};
    let now=u64::try_from(Utc::now().timestamp()).ok()?;
    wire::prepare(raw,&terms,U256::from_be_bytes(salt),now)
}

#[path="competition.rs"]
mod competition;
pub(in crate::venues::bsc_dex) use competition::compete;

type Offer=(Quote,Option<Ticket>);

async fn candidate(p:&Parts,asset:&Asset,side:Side,qty:Decimal,firm:bool)->Option<Offer>{
    let c=&p.ctx;
    if send::gas_mode::read(c).ok()?!=send::gas_mode::GasMode::Ethereum{return None;}
    let rev=c.settings.generation();
    let (base,quote)=market(p,asset)?;
    let depth=p.tokenlon.rates(p,base,quote,side);
    if depth.is_empty(){return None;}
    let buy=side==Side::Buy;
    direct::taxes(&p.quoting(),asset).ok()?;
    let head=p.heads.fresh(c).ok()?;
    p.lag.check(c).ok()?;
    let cap=c.dec("tokenlon.gas_budget_native").ok()?;
    let ready=send::select(c,&p.stats).ok()?;
    if cap<=Decimal::ZERO || ready.is_empty() || send::tip(&ready).is_some(){return None;}
    let ticker=gas_ticker::read(c).ok()?;
    let (i,o)=if buy {(quote,base)}else{(base,quote)};
    let estimate=if firm{Decimal::ZERO}else{super::gas::indicative(p,head.number,i,o)?};
    // Лестница SDK хранит только измеренные размеры; окончательный запрос допускает и новый размер.
    let observed=depth.iter().find_map(|(n,rate)|(*n==qty).then_some(rate.price));
    if observed.is_none() {
        if !depth.iter().any(|(_,rate)| rate.min<=qty && qty<=rate.max) {return None;}
        if !firm {p.tokenlon.request_size(p,base,quote,side,qty);return None;}
    }
    let mut q=Quote {venue:c.venue(),asset:asset.clone(),side,qty,fillable_qty:qty,
        avg_price:observed.and_then(|r|if buy {Decimal::ONE.checked_div(r)}else{Some(r)}).unwrap_or(Decimal::ZERO),
        costs:vec![Cost::new(CostKind::Gas,estimate,ticker.clone(),false)],signal_route:None,at:Utc::now(),pick:Some(super::LABEL.into()),
        route:Some(DexRoute {mode:RouteMode::RfqIndicative,legs:vec![],block:head.number,gas_units:0,executable:false,exec_note:Some(super::LABEL.into())})};
    if !firm {return Some((q,None));}
    c.bound().ok()?;
    let key=wallet::signer::signer(c).ok()?;
    let m=profile::markets(c)?.into_iter().find(|m|m.base_token==base && m.quote_token==quote)?;
    let (db,dc)=tokio::try_join!(p.tokens.decimals(c,base),p.tokens.decimals(c,quote)).ok()?;
    let target=direct::ceil_units(qty,db)?;
    if num::to_decimal(target,db)?!=qty{return None;}
    let mut req=client::Query{base:m.base,quote:m.quote,side,amount:qty,base_dec:db,input:i,output:o};
    let issued=client::firm(c,&key,&req).await.ok()?;
    let mut prepared=prepare(c,&issued.raw_order,&key,i,o)?;
    if buy && prepared.proof.net<target {
        let next=gross_for_net(target,prepared.proof.fee)?;
        req.amount=num::to_decimal(next,db)?;
        if num::to_units(req.amount,db)?!=next{return None;}
        if !depth.iter().any(|(_,r)|r.min<=req.amount && req.amount<=r.max) {return None;}
        let fresh=client::firm(c,&key,&req).await.ok()?;
        prepared=prepare(c,&fresh.raw_order,&key,i,o)?;
    }
    let proof=&prepared.proof;
    if (buy && proof.net<target) || (!buy && proof.terms.spent!=target) || !super::timely(c,proof.expiry){return None;}
    if buy && !fits_principal(proof.terms.spent,dc,quote_stage::buy_budget(c.place.id,quote)) {return None;}
    let sig=key.sign_hash_sync(&prepared.sign_digest).ok()?;
    let data=prepared.calldata(&sig)?;
    let proof=prepared.proof;
    let (block,gas)=sim::run(c,&proof,&data).await.ok()?;
    let gas_limit=send::tx::with_margin(c,alloy_primitives::U64::from(gas),c.dec("send.gas_limit_margin_pct").ok()?).ok()?;
    let price=exec::tx_fee::price(&p.env(),&ready,None).await.ok()?;
    let max_fee=exec::gas_fund::need_wei(U256::ZERO,price.cap(),gas_limit,None);
    if max_fee>num::to_units(cap,18)? {return None;}
    let native_value=sim::value(&proof);
    let need=exec::gas_fund::need_wei(native_value,price.cap(),gas_limit,None);
    let need=exec::gas_fund::add_native_reserve(c,native_value,need).ok()?;
    exec::gas_fund::ensure(c,key.address(),need).await.ok()?;
    // Ethereum L1: тот же снимок type-2, что в резерве. Calldata не покидает локальную симуляцию.
    let send::tx_price::TxPrice::Eip1559(fees,base_fee)=price else{return None};
    let effective=base_fee.checked_add(U256::from(fees.tip))?.min(U256::from(fees.max_fee));
    let cost=num::to_decimal(effective.checked_mul(U256::from(gas))?,18)?;
    if cost>cap{return None;}
    let cash=if buy {num::to_decimal(proof.terms.spent,dc)?}else{num::to_decimal(proof.net,dc)?};
    q.avg_price=cash.checked_div(qty)?;
    q.costs=vec![Cost::new(CostKind::Gas,cost,ticker,false)];
    q.route=Some(DexRoute{mode:RouteMode::Rfq(proof.tx_hash),legs:vec![],block,gas_units:gas,executable:true,exec_note:Some(super::LABEL.into())});
    if rev!=c.settings.generation() || !super::timely(c,proof.expiry) {return None;}
    q.at=Utc::now();
    Some((q,Some(Ticket{proof,data,qty,side,decs:(db,dc),revision:rev})))
}

fn fits_principal(spent:U256,decimals:u8,principal:Option<Decimal>)->bool {
    principal.is_none_or(|cap|num::to_units(cap,decimals).is_some_and(|limit|spent<=limit))
}

fn gross_for_net(net:U256, fee:u16)->Option<U256>{
    if fee>=10000 || net.is_zero(){return None;}
    Some(net.checked_mul(U256::from(10000))?.div_ceil(U256::from(10000-fee)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buy_gross_covers_requested_net_by_exactly_one_minimal_unit() {
        for (target,fee) in [(40_000_000_000_000_000u64,30u16),(1_000_000_000,4),(101,30),(1,9999),(100,0)] {
            let want=U256::from(target);
            let gross=gross_for_net(want,fee).unwrap();
            let paid=|v:U256| v*U256::from(10000-fee)/U256::from(10000);
            assert!(paid(gross)>=want);
            assert!(paid(gross-U256::from(1))<want);
        }
        assert_eq!(gross_for_net(U256::ZERO,10),None);
        assert_eq!(gross_for_net(U256::from(100),10000),None);
        assert_eq!(gross_for_net(U256::MAX,30),None);
    }
}
