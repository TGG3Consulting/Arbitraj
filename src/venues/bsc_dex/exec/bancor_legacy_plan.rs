//! Bind a quoted v2.1 path to its current wallet module and original registry entry.
use super::{
    bancor_legacy_wallet::{abi::{ILegacyNetwork,ILegacyWallet}, Bridge, NATIVE, Proof, Terms},
    chain::Step, plan::{DirectCall, Plan},
};
use crate::{
    core::types::{PoolRef, PoolState, bancor_legacy::BancorLegacyState},
    gateway::errors::{ErrorCode,GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::call::canonical,
        pools::{dagrow,direct_module,family_variant::Variant,registry::Entry},
        quote::{v2_wallet_sim as sim,white_state},wallet::approve},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address,B256,Bytes,U256};
use serde_json::Value;

fn bad(ctx:&Ctx)->crate::gateway::errors::GatewayError {
    ctx.err(ErrorCode::NoRoute,"Bancor V2.1: изменились конвертер, якорь, валюта или модуль")
}
pub(crate) fn state(e:&Entry)->Option<&BancorLegacyState>{
    let PoolState::BancorLegacy(s)=&e.pool.state else{return None;};
    let shape=match (&s.second,&e.pool.reference) {
        (None,PoolRef::Contract(owner))=>*owner==s.converter,
        (Some(b),PoolRef::Singleton{manager,id})=>*manager==s.converter && *id==b.converter.into_word() &&
            s.tokens==[e.pool.token0,e.pool.token1] && s.tokens[0]<s.tokens[1] &&
            !b.middle.is_zero() && b.middle!=NATIVE && !s.reserves.contains(&b.middle) &&
            b.converter!=s.converter && !b.converter.is_zero() && b.anchor!=s.anchor && !b.anchor.is_zero(),
        _=>false,
    };
    (e.family.variant==Variant::BancorLegacy && shape &&
     !s.anchor.is_zero() && !s.network.is_zero() && !s.converter.is_zero() && s.tokens[0]!=s.tokens[1] &&
     (s.tokens==[e.pool.token0,e.pool.token1] || s.tokens==[e.pool.token1,e.pool.token0])).then_some(s)
}
pub(crate) fn step_id(e:&Entry,input:Address)->Option<B256> {
    let state=state(e)?;
    if state.second.is_some(){return None;} // kind 17 encodes one anchor; V5 uses the two original steps.
    let path=state.path(input)?;
    let mut id=path[1].into_word();
    id.0[11]=u8::from(path[0]==NATIVE) | (u8::from(path[2]==NATIVE)<<1);
    Some(id)
}
pub(crate) fn request(ctx:&Ctx,e:&Entry,input:Address,amount:U256)->GatewayResult<(Address,Bytes)> {
    let s=state(e).ok_or_else(||bad(ctx))?;
    let path=s.path(input).ok_or_else(||bad(ctx))?;
    Ok((s.network,ILegacyNetwork::rateByPathCall{path,amount}.abi_encode().into()))
}
pub(crate) async fn official(ctx:&Ctx,e:&Entry,input:Address,amount:U256,at:&Value)->GatewayResult<U256> {
    let s=state(e).ok_or_else(||bad(ctx))?;
    let path=s.path(input).ok_or_else(||bad(ctx))?;
    canonical(ctx,at,s.network,ILegacyNetwork::rateByPathCall{path,amount}).await
}
pub(crate) fn terms(ctx:&Ctx,e:&Entry,target:Address,native:bool)->GatewayResult<Terms> {
    let s=state(e).ok_or_else(||bad(ctx))?;
    let pins=direct_module::configured(ctx,e.family.id)?;
    let [module_hash]=pins.as_slice() else{return Err(bad(ctx));};
    if target.is_zero() || target!=ctx.dex_addr("dex.router",e.family.id)? ||
        (native && !s.reserves.contains(&NATIVE)) {return Err(bad(ctx));}
    for i in 0..2 {
        let normalized=if s.reserves[i]==NATIVE {ctx.addr("dex.native_wrapped")?} else {s.reserves[i]};
        if normalized!=s.tokens[i] || normalized.is_zero() {return Err(bad(ctx));}
    }
    let bridge=s.second.as_ref().map(|b|Bridge{middle:b.middle,anchor:b.anchor,converters:[s.converter,b.converter]});
    Ok(Terms{target,network:s.network,normalized:s.tokens,reserves:s.reserves,anchor:s.anchor,bridge,native,module_hash:*module_hash})
}
pub(crate) async fn verify(ctx:&Ctx,e:&Entry,input:Address,t:Terms,at:&Value)->GatewayResult<()> {
    let check_path=async {
        let path=t.path(input).ok_or_else(||bad(ctx))?;
        if t.bridge.is_some() {
            let fresh=Box::pin(super::super::pools::bancor_legacy_bridge::bound(ctx,e.family,&e.pool.reference,
                (e.pool.token0,e.pool.token1),at)).await?;
            if fresh.path(input).as_deref()!=Some(path.as_slice()) || fresh.network!=t.network{return Err(bad(ctx));}
        } else {
            let owner=canonical(ctx,at,t.anchor,ILegacyNetwork::ownerCall{}).await?;
            if e.pool.reference!=PoolRef::Contract(owner){return Err(bad(ctx));}
        }
        Ok(())
    };
    let (_,pools,network,wrapped,())=tokio::try_join!(
        direct_module::verify_code(ctx,e.family.id,t.target,at),
        canonical(ctx,at,t.target,ILegacyWallet::poolsCall{}),
        canonical(ctx,at,t.target,ILegacyWallet::networkCall{}),
        canonical(ctx,at,t.target,ILegacyWallet::wrappedCall{}),
        check_path)?;
    if pools!=ctx.dex_addr("dex.factory",e.family.id)? || network!=t.network ||
       wrapped!=ctx.addr("dex.native_wrapped")? {return Err(bad(ctx));}
    Ok(())
}
pub(super) async fn check_step(ctx:&Ctx,e:&Entry,input:Address,at:&Value)->GatewayResult<()> {
    let t=terms(ctx,e,ctx.dex_addr("dex.router",e.family.id)?,false)?;
    verify(ctx,e,input,t,at).await
}
pub(super) async fn plan(ctx:&Ctx,s:&Step,payer:Address,height:u64,pick:Option<(Address,bool)>)->GatewayResult<Plan> {
    if !s.from_wallet || !s.to_wallet{return Err(bad(ctx));}
    let generation=ctx.settings.generation();
    let (target,native)=pick.ok_or_else(||bad(ctx))?;
    let o=&s.order;
    let t=terms(ctx,&s.entry,target,native)?;
    let h=white_state::head(ctx,&format!("{height:#x}")).await?;
    let at=h.selector();
    verify(ctx,&s.entry,o.token_in,t,&at).await?;
    let order=sim::Order{payer,target,input:o.token_in,output:o.token_out,amount:o.amount_in,minimum:o.min_out,wire:sim::Wire::BancorLegacy(Box::new(t))};
    let deadline=U256::from(h.timestamp.to::<u64>().checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||bad(ctx))?);
    let proof=t.proof(&order,o.min_out,deadline);
    sim::measure(ctx,h,order,true).await?;
    let approvals=if t.value(o.token_in,o.amount_in).is_zero(){
        let allowance=dagrow::allowance(ctx,o.token_in,payer,target,&at).await?;
        approve::fork_exact(ctx,o.token_in,target,Some(allowance),o.amount_in)?
    }else{vec![]};
    if ctx.settings.generation()!=generation{return Err(bad(ctx));}
    let mut plan=Plan::new(ctx,(approvals,target,payer),o.token_out,vec![],o.min_out,deadline)?;
    plan.set_direct_market(DirectCall::BancorLegacy(std::sync::Arc::new(proof)));
    Ok(plan)
}
pub(crate) fn probe(ctx:&Ctx,e:&Entry,o:&super::plan::Order,payer:Address,now:u64)->GatewayResult<(Vec<approve::Approval>,Address,Bytes)> {
    let target=ctx.dex_addr("dex.router",e.family.id)?;
    let t=terms(ctx,e,target,false)?;
    let proof=Proof{terms:t,payer,input:o.token_in,output:o.token_out,maximum:o.amount_in,minimum:o.min_out,
        deadline:U256::from(now.checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||bad(ctx))?)};
    Ok((approve::fork_exact(ctx,o.token_in,target,None,o.amount_in)?,target,proof.data()))
}
