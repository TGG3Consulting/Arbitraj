//! Собственный nonce и хеш calldata сохраняются до первого канала отправки. Подпись не сохраняется в журнале.
use super::{receipt::Proof,routing::Ticket,sim,wire};
use crate::{
    core::types::{Side,TradeRequest,TradeResult,TxOutcome},
    gateway::{errors::{ErrorCode,GatewayResult},events::MarketEvent},
    venues::bsc_dex::{ctx::Ctx,exec,hashflow,num,parts::Parts,send::track::Receipt,wallet},
};
use alloy_primitives::{B256,U256,keccak256};
use rust_decimal::Decimal;
use serde::{Deserialize,Serialize};

#[derive(Debug,Clone,Serialize,Deserialize)]
pub(in crate::venues::bsc_dex) struct Saved {
    pub(crate) order:Proof,
    pub(crate) calldata:B256,
    pub(super) from_block:u64,
}
impl Saved {
    pub(in crate::venues::bsc_dex) fn wallet(&self,ctx:&Ctx)->bool {
        ctx.chain_id()==Some(1) && wallet::signer::address(ctx).ok()==Some(self.order.terms.payer)
    }
    pub(in crate::venues::bsc_dex) fn signed(&self)->exec::signed_call::Erc20Call {
        exec::signed_call::Erc20Call{chain:1,payer:self.order.terms.payer,module:wire::USER_PROXY,calldata:self.calldata}
    }
    pub(in crate::venues::bsc_dex) fn amounts(&self,logs:&[alloy::rpc::types::Log])->Option<(U256,U256)> {
        let mut accounting=self.order.clone();
        accounting.terms.minimum=U256::from(1);
        accounting.amounts(logs)
    }
    pub(in crate::venues::bsc_dex) async fn included(&self,ctx:&Ctx,hash:B256,nonce:u64,r:&Receipt)->GatewayResult<()> {
        if r.status.to::<u64>()==1 {
            let mut accounting=self.order.clone();
            accounting.terms.minimum=U256::from(1);
            accounting.included(ctx,hash,nonce,self.calldata,r).await.map(|_|())
        } else if r.status.to::<u64>()==0 {
            self.order.transaction_matches(ctx,hash,nonce,self.calldata,r).await
        } else {
            Err(ctx.err(ErrorCode::OutcomeUnknown,"Некорректный статус исходной квитанции Tokenlon"))
        }
    }
}
pub(in crate::venues::bsc_dex) async fn trade(p:&Parts,req:TradeRequest,t:&Ticket)->GatewayResult<TradeResult>{
    let c=&p.ctx;
    c.bound()?;
    let fail=||c.err(ErrorCode::NoRoute,"Tokenlon: изменились условия подтверждённого поручения");
    let owner=wallet::signer::address(c)?;
    let valid=|| super::enabled(c) && c.settings.generation()==t.revision && super::timely(c,t.proof.expiry);
    let coin=req.asset.contract.ok_or_else(fail)?;
    let cash=p.tokens.usdt(c)?;
    let buy=req.side==Side::Buy;
    let pair=if buy {(cash,coin)}else{(coin,cash)};
    let terms=&t.proof.terms;
    if !valid() || owner!=terms.payer || req.venue!=c.venue() || req.qty!=t.qty || req.side!=t.side
        || pair!=(terms.input,terms.output) || req.worst_price<=Decimal::ZERO
        || (coin==wire::WETH)!=exec::native::is_native(c,&req.asset) || req.remeasure.as_ref().is_some_and(|r|!r.any_executor) {return Err(fail());}
    let cost=req.qty.checked_mul(req.worst_price).ok_or_else(fail)?;
    let (coin_dec,cash_dec)=t.decs;
    let mut proof=t.proof.clone();
    if buy {
        if num::to_decimal(terms.spent,cash_dec).is_none_or(|v|v>cost) || num::to_decimal(t.proof.net,coin_dec).is_none_or(|v|v<req.qty) {return Err(fail());}
        proof.terms.minimum=super::super::quote::direct::ceil_units(req.qty,coin_dec).ok_or_else(fail)?;
    } else {
        let minimum=req.remeasure.as_ref().map_or(cost,|v|v.floor.max(cost));
        if num::to_decimal(terms.spent,coin_dec)!=Some(req.qty) || num::to_decimal(t.proof.net,cash_dec).is_none_or(|v|v<minimum) {return Err(fail());}
        proof.terms.minimum=super::super::quote::direct::ceil_units(minimum,cash_dec).ok_or_else(fail)?;
    }
    let env=p.env();
    let hold=env.inflows.swap(c,pair.1,&|text|env.stats.emit(MarketEvent::Notice{
        venue:c.venue(),code:"dex.swap_waits_inflow".into(),text})).await?;
    let slot=p.nonces.acquire(c).await?;
    if slot.from!=owner || !valid(){return Err(fail());}
    let (from_block,gas)=sim::run(c,&proof,&t.data).await?;
    if !valid(){return Err(fail());}
    let saved=Saved{order:proof,calldata:keccak256(&t.data),from_block};
    let mut parse=exec::swap_ctx::SwapCtx::swap(owner,pair,t.decs,&[],None,None);
    parse.tokenlon=Some(Box::new(saved.clone()));
    parse.who=Some(super::LABEL.into());
    let raw=exec::txs::send(&env,&slot,wire::USER_PROXY,t.data.clone(),sim::value(&t.proof),false,Some(hold));
    let measured=super::private_node::Measurement{owner,digest:saved.calldata,value:sim::value(&saved.order),gas};
    let done=super::private_node::MEASUREMENT.scope(measured,hashflow::native_send(p,&req,t.proof.expiry,t.revision,&parse,raw)).await?;
    exec::market_receipt::check_module_cancel(c,&parse,&done).await.map_err(|e|parse.attach(e.with_tx(done.chain.clone())))?;
    if matches!(done.chain.outcome,TxOutcome::Included|TxOutcome::Reverted){
        let r=done.receipt.as_ref().ok_or_else(||parse.attach(c.err(ErrorCode::OutcomeUnknown,"Нет собственной квитанции Tokenlon").with_tx(done.chain.clone())))?;
        saved.included(c,done.chain.hash,done.chain.nonce,r).await.map_err(|e|parse.attach(e.with_tx(done.chain.clone())))?;
    }
    exec::result::by(exec::result::with_native_fact(c,req,done,owner,pair,t.decs,&[],Some(&parse)).map_err(|e|parse.attach(e)),super::LABEL)
}
