use super::{aktionariat_wallet::{IAktionariatWallet, Market, Proof}, chain::Step, plan::{DirectCall,Plan}};
use crate::{
    gateway::errors::{ErrorCode,GatewayResult},
    venues::bsc_dex::{ctx::Ctx,node::call::canonical,pools::{brokerbot,dagrow,direct_module,registry::Entry},
        quote::{v2_wallet_sim as sim,white_state},wallet::approve},
};
use alloy_primitives::{Address,Bytes,U256};
use serde_json::Value;

fn changed(ctx:&Ctx)->crate::gateway::errors::GatewayError {
    ctx.err(ErrorCode::NoRoute,"Aktionariat: изменилась привязка рынка, валют или платёжного модуля")
}
pub(crate) fn terms(ctx:&Ctx,e:&Entry)->GatewayResult<Market>{
    let module=ctx.dex_addr("dex.router",e.family.id)?;
    let pins=direct_module::configured(ctx,e.family.id)?;
    let [runtime]=pins.as_slice() else{return Err(changed(ctx));};
    let crate::core::types::PoolState::Brokerbot(s)=&e.pool.state else{return Err(changed(ctx));};
    let broker=e.pool.reference.address();
    if module.is_zero() || broker!=s.broker || ![e.pool.token0,e.pool.token1].contains(&s.share) ||
        ![e.pool.token0,e.pool.token1].contains(&s.cash) || s.share==s.cash {return Err(changed(ctx));}
    Ok(Market{broker,module,runtime:*runtime})
}
pub(crate) async fn verify(ctx:&Ctx,e:&Entry,m:Market,at:&Value)->GatewayResult<()> {
    let (state,(),registry,issuer)=tokio::try_join!(
        brokerbot::market(ctx,e.family,m.broker,(e.pool.token0,e.pool.token1),at),
        direct_module::verify_code(ctx,e.family.id,m.module,at),
        canonical(ctx,at,m.module,IAktionariatWallet::registryCall{}),
        canonical(ctx,at,m.module,IAktionariatWallet::issuerHubCall{}))?;
    if registry!=ctx.dex_addr("dex.factory",e.family.id)? || issuer!=ctx.dex_addr("dex.quoter",e.family.id)? {return Err(changed(ctx));}
    if !state.registered && !(state.hub==issuer &&
        canonical(ctx,at,m.module,IAktionariatWallet::issuerMarketsCall{market:m.broker}).await?) {return Err(changed(ctx));}
    Ok(())
}
pub(super) async fn plan(ctx:&Ctx,s:&Step,payer:Address,height:u64,selected:Option<Address>)->GatewayResult<Plan>{
    let generation=ctx.settings.generation();
    let m=terms(ctx,&s.entry)?;
    if !s.from_wallet || !s.to_wallet || selected!=Some(m.module) {return Err(changed(ctx));}
    let h=white_state::head(ctx,&format!("{height:#x}")).await?;
    let at=h.selector();
    verify(ctx,&s.entry,m,&at).await?;
    let o=&s.order;
    let deadline=U256::from(h.timestamp.to::<u64>().checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||changed(ctx))?);
    let order=sim::Order{payer,target:m.module,input:o.token_in,output:o.token_out,amount:o.amount_in,
        minimum:o.min_out,wire:sim::Wire::Aktionariat(m)};
    let proof=m.proof(&order,o.min_out,deadline);
    let (_,allowance)=tokio::try_join!(sim::measure(ctx,h,order,true),dagrow::allowance(ctx,o.token_in,payer,m.module,&at))?;
    let approvals=approve::fork_exact(ctx,o.token_in,m.module,Some(allowance),o.amount_in)?;
    if ctx.settings.generation()!=generation{return Err(changed(ctx));}
    let mut plan=Plan::new(ctx,(approvals,m.module,payer),o.token_out,vec![],o.min_out,deadline)?;
    plan.set_direct_market(DirectCall::Aktionariat(std::sync::Arc::new(proof)));
    Ok(plan)
}
pub(crate) fn probe(ctx:&Ctx,e:&Entry,o:&super::plan::Order,payer:Address,now:u64)->GatewayResult<(Vec<approve::Approval>,Address,Bytes)>{
    let market=terms(ctx,e)?;
    let proof=Proof{market,payer,input:o.token_in,output:o.token_out,budget:o.amount_in,minimum:o.min_out,v5:None,
        deadline:U256::from(now.checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||changed(ctx))?)};
    Ok((approve::fork_exact(ctx,o.token_in,market.module,None,o.amount_in)?,market.module,proof.data()))
}

pub(super) fn capture(ctx:&Ctx,legs:&[Vec<Step>],call:&super::executor::call::ExecCall,payer:Address)->GatewayResult<Option<Box<Proof>>>{
    let [leg]=legs else{return Ok(None)};
    let [step]=leg.as_slice() else{return Ok(None)};
    if !step.entry.family.variant.issuer_shares(){return Ok(None);}
    let m=terms(ctx,&step.entry)?;
    let [route]=call.steps.as_slice() else{return Err(changed(ctx));};
    let id=super::aktionariat_wallet::step_id(m.broker,Some(call.amount_in)).ok_or_else(||changed(ctx))?;
    if call.native.is_some() || route.kind!=17 || route.shareBps!=10000 || route.pool!=m.module ||
        route.data.as_ref()!=id.as_slice() || (route.tokenIn,route.tokenOut)!=(call.token_in,call.token_out){
        return Err(changed(ctx));
    }
    let signed=super::signed_call::Erc20Call{chain:1,payer,module:call.executor,calldata:alloy_primitives::keccak256(call.data())};
    Ok(Some(Box::new(Proof{market:m,payer,input:call.token_in,output:call.token_out,budget:call.amount_in,
        minimum:call.min_out,deadline:call.deadline,v5:Some(signed)})))
}
