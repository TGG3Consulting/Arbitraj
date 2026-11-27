//! Official Carbon v6 getters settle the same ordered (strategyId,amount) vector selected by the local quote.
use super::{carbon::{self,ICarbon,TradeAction},registry::Entry};
use super::super::{ctx::Ctx,math::carbon_match::{self,Plan},node::rpc};
use crate::{core::types::{CarbonState,PoolState},gateway::errors::{ErrorCode,GatewayResult}};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address,B256,Bytes,U256};
use serde_json::{Value,json};

pub(crate) fn contracts(s:&CarbonState,input:Address)->Option<(Address,Address)>{
    let raw=carbon::raw(s);if input==s.tokens[0]{Some((raw[0],raw[1]))}else if input==s.tokens[1]{Some((raw[1],raw[0]))}else{None}
}
fn payload(s:&CarbonState,input:Address,p:&Plan)->Option<Bytes>{
    let (source,target)=contracts(s,input)?;
    let actions=p.actions.iter().map(|a|TradeAction{strategyId:a.id,amount:a.amount}).collect();
    Some(if p.by_target{ICarbon::calculateTradeSourceAmountCall{sourceToken:source,targetToken:target,actions}.abi_encode().into()}else{ICarbon::calculateTradeTargetAmountCall{sourceToken:source,targetToken:target,actions}.abi_encode().into()})
}
fn decoded(p:&Plan,data:&[u8])->Option<u128>{
    let (actual,expected)=if p.by_target{(ICarbon::calculateTradeSourceAmountCall::abi_decode_returns_validate(data).ok()?,p.spent)}else{(ICarbon::calculateTradeTargetAmountCall::abi_decode_returns_validate(data).ok()?,p.received)};
    (actual==expected && actual>0).then_some(actual)
}
pub(crate) async fn official(ctx:&Ctx,s:&CarbonState,input:Address,p:&Plan,at:&Value)->GatewayResult<u128>{
    let bad=||ctx.err(ErrorCode::QuoteMismatch,"Carbon: официальный расчёт распределения по стратегиям расходится с локальным");
    if at["requireCanonical"]!=true || at["blockHash"].as_str().and_then(|v|v.parse::<B256>().ok()).is_none(){return Err(bad());}
    let data=payload(s,input,p).ok_or_else(bad)?;let raw:Bytes=rpc::call(ctx,"eth_call",json!([{"to":s.controller,"data":data},at])).await?;
    decoded(p,&raw).ok_or_else(bad)
}
fn quote(e:&Entry,input:Address,amount:U256)->Option<(&CarbonState,Plan)>{
    let PoolState::Carbon(s)=&e.pool.state else{return None};let amount=u128::try_from(amount).ok()?;
    let p=carbon_match::plan(s,input,false,amount)?;(p.spent==amount).then_some((s,p))
}
pub(crate) fn request(ctx:&Ctx,e:&Entry,input:Address,amount:U256)->GatewayResult<(Address,Bytes)>{
    let bad=||ctx.err(ErrorCode::NoRoute,"Carbon: недостаточно исполнимых заявок на полную сумму");
    let(s,p)=quote(e,input,amount).ok_or_else(bad)?;let data=payload(s,input,&p).ok_or_else(bad)?;Ok((s.controller,data))
}
pub(crate) fn payout(e:&Entry,input:Address,amount:U256,raw:&[u8])->Option<U256>{let(_,p)=quote(e,input,amount)?;decoded(&p,raw).map(U256::from)}
