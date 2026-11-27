use super::{chain::Step,plan::{DirectCall,Plan},yield_basis_wallet::{abi::IYieldWallet,Proof,Terms}};
use crate::{gateway::errors::{ErrorCode,GatewayResult},
    venues::bsc_dex::{ctx::Ctx,node::call::canonical,pools::{dagrow,direct_module,family_variant::Variant,registry::Entry,yield_basis_quotes},
                      quote::{v2_wallet_sim as sim,white_state},wallet::approve}};
use alloy_primitives::{Address,Bytes,U256};
use serde_json::Value;

fn bad(ctx:&Ctx)->crate::gateway::errors::GatewayError {
    ctx.err(ErrorCode::NoRoute,"YieldBasis: рынок, валюты или модуль кошелька не совпали")
}
pub(crate) fn terms(ctx:&Ctx,e:&Entry,target:Address,native:bool)->GatewayResult<Terms> {
    let (market,stable)=yield_basis_quotes::shape(e).ok_or_else(||bad(ctx))?;
    let pins=direct_module::configured(ctx,e.family.id)?;
    let [module_hash]=pins.as_slice() else{return Err(bad(ctx));};
    let wrapped=ctx.addr("dex.native_wrapped")?;
    if e.family.variant!=Variant::YieldBasis || target!=ctx.dex_addr("dex.router",e.family.id)? || target.is_zero() ||
       (native && ![e.pool.token0,e.pool.token1].contains(&wrapped)){return Err(bad(ctx));}
    Ok(Terms{market,stable,module_hash:*module_hash,target,wrapped,native})
}
pub(crate) async fn check_module(ctx:&Ctx,e:&Entry,t:Terms,at:&Value)->GatewayResult<()> {
    let ((),factory,wrapped)=tokio::try_join!(direct_module::verify_code(ctx,e.family.id,t.target,at),
        canonical(ctx,at,t.target,IYieldWallet::factoryCall{}),canonical(ctx,at,t.target,IYieldWallet::wrappedCall{}))?;
    if factory!=ctx.dex_addr("dex.factory",e.family.id)? || wrapped!=t.wrapped{return Err(bad(ctx));}
    Ok(())
}
pub(super) async fn plan(ctx:&Ctx,s:&Step,payer:Address,height:u64,pick:Option<(Address,bool)>)->GatewayResult<Plan> {
    let generation=ctx.settings.generation();
    if !s.from_wallet || !s.to_wallet{return Err(bad(ctx));}
    let (target,native)=pick.ok_or_else(||bad(ctx))?;
    let e=&s.entry;
    let t=terms(ctx,e,target,native)?;
    let o=&s.order;
    let h=white_state::head(ctx,&format!("{height:#x}")).await?;
    let at=h.selector();
    let (registered,())=tokio::try_join!(yield_basis_quotes::bound(ctx,e.family,&e.pool.reference,(e.pool.token0,e.pool.token1),&at),
                                        check_module(ctx,e,t,&at))?;
    if registered.stable!=t.stable{return Err(bad(ctx));}
    let order=sim::Order{payer,target,input:o.token_in,output:o.token_out,amount:o.amount_in,minimum:o.min_out,wire:sim::Wire::YieldBasis(t)};
    let deadline=U256::from(h.timestamp.to::<u64>().checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||bad(ctx))?);
    let proof=t.proof(&order,o.min_out,deadline);
    sim::measure(ctx,h,order,true).await?;
    let approvals=if t.value(o.token_in,o.amount_in).is_zero(){
        let current=dagrow::allowance(ctx,o.token_in,payer,target,&at).await?;
        approve::fork_exact(ctx,o.token_in,target,Some(current),o.amount_in)?
    }else{vec![]};
    if ctx.settings.generation()!=generation{return Err(bad(ctx));}
    let mut plan=Plan::new(ctx,(approvals,target,payer),o.token_out,vec![],o.min_out,deadline)?;
    plan.set_direct_market(DirectCall::YieldBasis(std::sync::Arc::new(proof)));
    Ok(plan)
}
pub(crate) fn probe(ctx:&Ctx,e:&Entry,o:&super::plan::Order,payer:Address,now:u64)->GatewayResult<(Vec<approve::Approval>,Address,Bytes)> {
    let target=ctx.dex_addr("dex.router",e.family.id)?;
    let t=terms(ctx,e,target,false)?;
    let proof=Proof{terms:t,payer,input:o.token_in,output:o.token_out,maximum:o.amount_in,minimum:o.min_out,
        deadline:U256::from(now.checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||bad(ctx))?)};
    Ok((approve::fork_exact(ctx,o.token_in,target,None,o.amount_in)?,target,proof.data()))
}
