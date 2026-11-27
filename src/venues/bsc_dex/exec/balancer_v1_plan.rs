use super::{
    balancer_v1_wallet::{IBPoolWallet, Proof, Terms},
    chain::Step,plan::{DirectCall, Plan},
};
use crate::{core::types::PoolRef, gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::call::canonical,
        pools::{dagrow, direct_module, registry::Entry, solidly_classic_factory::plain},
        quote::{v2_wallet_sim as sim, white_state}, wallet::approve}};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::Value;

alloy::sol! { interface IBPoolFactory { function isBPool(address pool) external view returns(bool); } }

#[derive(Clone,Copy)]
pub(crate) struct Selection {pub target:Address,pub native:bool,pub desired:Option<U256>}
fn bad(ctx:&Ctx)->crate::gateway::errors::GatewayError {
    ctx.err(ErrorCode::NoRoute,"Balancer V1: изменились рынок, кошелёк или форма обмена")
}
fn digest(ctx:&Ctx,id:&str,key:&str)->GatewayResult<B256>{
    ctx.dex_text(key,id)?.parse().map_err(|_|bad(ctx))
}
pub(crate) fn terms(ctx:&Ctx,e:&Entry,pick:Selection)->GatewayResult<Terms>{
    let PoolRef::Multi{contract:pool,token0,token1}=&e.pool.reference else{return Err(bad(ctx));};
    let id=e.family.id;
    let wrapped=ctx.addr("dex.native_wrapped")?;
    let hashes=direct_module::configured(ctx,id)?;
    let [pin]=hashes.as_slice() else{return Err(bad(ctx));};
    if pick.target!=ctx.dex_addr("dex.router",id)? || pick.target.is_zero() || pick.target==*pool ||
        [*token0,*token1]!=[e.pool.token0,e.pool.token1] || (pick.native && ![*token0,*token1].contains(&wrapped)){
        return Err(bad(ctx));
    }
    Ok(Terms{pool:*pool,target:pick.target,wrapped,native:pick.native,desired:pick.desired,
        module_hash:*pin,pool_hash:digest(ctx,id,"dex.pair_runtime_hash")?})
}
pub(crate) async fn check(ctx:&Ctx,e:&Entry,t:&Terms,at:&Value)->GatewayResult<()>{
    let id=e.family.id;
    let factory=ctx.dex_addr("dex.factory",id)?;
    let factory_hash=digest(ctx,id,"dex.factory_runtime_hash")?;
    let ((),(),(),origin,wrapped,listed)=tokio::try_join!(direct_module::verify_code(ctx,id,t.target,at),
        plain(ctx,factory,factory_hash,at),plain(ctx,t.pool,t.pool_hash,at),
        canonical(ctx,at,t.target,IBPoolWallet::factoryCall{}),canonical(ctx,at,t.target,IBPoolWallet::wrappedCall{}),
        canonical(ctx,at,factory,IBPoolFactory::isBPoolCall{pool:t.pool}))?;
    if origin!=factory || wrapped!=t.wrapped || !listed {return Err(bad(ctx));}
    Ok(())
}
pub(super) async fn plan(ctx:&Ctx,s:&Step,payer:Address,height:u64,pick:Option<Selection>)->GatewayResult<Plan>{
    let g=ctx.settings.generation();
    if !s.from_wallet || !s.to_wallet{return Err(bad(ctx));}
    let t=terms(ctx,&s.entry,pick.ok_or_else(||bad(ctx))?)?;
    let o=&s.order;
    let h=white_state::head(ctx,&format!("{height:#x}")).await?;
    let at=h.selector();
    check(ctx,&s.entry,&t,&at).await?;
    let order=sim::Order{payer,target:t.target,input:o.token_in,output:o.token_out,amount:o.amount_in,
        minimum:o.min_out,wire:sim::Wire::BalancerV1(t)};
    let deadline=U256::from(h.timestamp.to::<u64>().checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||bad(ctx))?);
    let proof=t.proof(&order,o.min_out,deadline);
    sim::measure(ctx,h,order,true).await?;
    let approvals=if t.value(o.token_in,o.amount_in).is_zero(){
        let current=dagrow::allowance(ctx,o.token_in,payer,t.target,&at).await?;
        approve::fork_exact(ctx,o.token_in,t.target,Some(current),o.amount_in)?
    }else{vec![]};
    if ctx.settings.generation()!=g{return Err(bad(ctx));}
    let mut plan=Plan::new(ctx,(approvals,t.target,payer),o.token_out,vec![],o.min_out,deadline)?;
    plan.set_direct_market(DirectCall::BalancerV1(std::sync::Arc::new(proof)));
    Ok(plan)
}
pub(crate) fn probe(ctx:&Ctx,e:&Entry,o:&super::plan::Order,payer:Address,now:u64)->GatewayResult<(Vec<approve::Approval>,Address,Bytes)>{
    let target=ctx.dex_addr("dex.router",e.family.id)?;
    let t=terms(ctx,e,Selection{target,native:false,desired:None})?;
    let p=Proof{terms:t,payer,input:o.token_in,output:o.token_out,maximum:o.amount_in,minimum:o.min_out,
        deadline:U256::from(now.checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||bad(ctx))?)};
    Ok((approve::fork_exact(ctx,o.token_in,target,None,o.amount_in)?,target,p.data()))
}
