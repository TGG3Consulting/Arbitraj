use super::{bancor_v3_wallet::{IBancorTrade,Terms},chain::Step,plan::{DirectCall,Plan}};
use crate::{core::types::bancor_v3::BancorState,gateway::errors::{ErrorCode,GatewayResult},venues::bsc_dex::{
    ctx::Ctx,node::call::canonical,pools::{bancor_v3,dagrow,direct_module,exact_wallet_tokens,registry::Entry},
    quote::{v2_wallet_sim as sim,white_state},wallet::approve,
}};
use alloy_primitives::{Address,Bytes,U256};
use serde_json::Value;

fn bad(ctx:&Ctx)->crate::gateway::errors::GatewayError{ctx.err(ErrorCode::NoRoute,"Bancor V3: рынок, плательщик или денежный предел изменились")}
#[derive(Clone,Copy)]
pub(crate) struct Form{pub module:bool,pub native:bool,pub exempt:bool,pub desired:Option<U256>}
pub(crate) fn terms(ctx:&Ctx,e:&Entry,s:&BancorState,input:Address,f:Form)->GatewayResult<Terms>{
    let id=e.family.id;
    let eth=ctx.dex_addr("dex.bancor_native_token",id)?;
    let side=s.side(input).ok_or_else(||bad(ctx))?;
    let source=s.physical[side];let target=s.physical[1-side];
    let uses_eth=source==eth || target==eth;
    if f.native && !uses_eth{return Err(bad(ctx));}
    let (contract,module_hash)=if f.module{
        let module=ctx.dex_addr("dex.router",id)?;
        let hashes=direct_module::configured(ctx,id)?;
        let [pin]=hashes.as_slice() else{return Err(bad(ctx));};
        if module.is_zero() || [s.network,s.info,s.vault,source,target].contains(&module){return Err(bad(ctx));}
        (module,Some(*pin))
    }else{
        if uses_eth!=f.native || [source,target].iter().any(|t|*t!=eth && !exact_wallet_tokens::eligible_input(ctx,id,*t)){return Err(bad(ctx));}
        (s.network,None)
    };
    Ok(Terms{network:s.network,info:s.info,vault:s.vault,wrapped:s.wrapped,eth,source,target,contract,
        network_code:bancor_v3::role(ctx,e.family,s.network)?,vault_code:bancor_v3::role(ctx,e.family,s.vault)?,
        module_hash,native_wallet:f.native,exemption:f.exempt,desired:f.desired})
}

pub(crate) async fn check(ctx:&Ctx,e:&Entry,s:&BancorState,t:&Terms,payer:Address,at:&Value)->GatewayResult<()>{
    let actor=if t.module_hash.is_some(){t.contract}else{payer};
    if t.exemption && !bancor_v3::arb(ctx,s,actor,at).await?{return Err(bad(ctx));}
    if t.module_hash.is_some(){
        let m=t.contract;
        let ((),network,info,vault,wrapped,native)=tokio::try_join!(direct_module::verify_code(ctx,e.family.id,m,at),
            canonical(ctx,at,m,IBancorTrade::networkCall{}),canonical(ctx,at,m,IBancorTrade::infoCall{}),
            canonical(ctx,at,m,IBancorTrade::vaultCall{}),canonical(ctx,at,m,IBancorTrade::wrappedCall{}),canonical(ctx,at,m,IBancorTrade::nativeTokenCall{}))?;
        if (network,info,vault,wrapped,native)!=(s.network,s.info,s.vault,s.wrapped,t.eth){return Err(bad(ctx));}
    }else{
        for token in [t.source,t.target]{if token!=t.eth{exact_wallet_tokens::check_input(ctx,e.family.id,token,at).await?;}}
    }
    Ok(())
}

pub(super) async fn plan(ctx:&Ctx,s:&Step,payer:Address,height:u64,selected:Option<(Address,Form)>)->GatewayResult<Plan>{
    let generation=ctx.settings.generation();
    let (target,form)=selected.ok_or_else(||bad(ctx))?;
    if !s.from_wallet || !s.to_wallet{return Err(bad(ctx));}
    let h=white_state::head(ctx,&format!("{height:#x}")).await?;let at=h.selector();
    let e=&s.entry;let o=&s.order;
    let state=bancor_v3::load(ctx,e.family,&e.pool.reference,[e.pool.token0,e.pool.token1],&at).await?;
    let t=terms(ctx,e,&state,o.token_in,form)?;
    if target!=t.contract{return Err(bad(ctx));}
    check(ctx,e,&state,&t,payer,&at).await?;
    let order=sim::Order{payer,target,input:o.token_in,output:o.token_out,amount:o.amount_in,minimum:o.min_out,wire:sim::Wire::BancorV3(Box::new(t.clone()))};
    let deadline=U256::from(h.timestamp.to::<u64>().checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||bad(ctx))?);
    let proof=t.proof(&order,o.min_out,deadline);
    let approvals=if t.value(o.amount_in).is_zero(){
        let current=dagrow::allowance(ctx,o.token_in,payer,target,&at).await?;
        approve::fork_exact(ctx,o.token_in,target,Some(current),o.amount_in)?
    }else{vec![]};
    sim::measure(ctx,h,order,true).await?;
    if ctx.settings.generation()!=generation{return Err(bad(ctx));}
    let mut plan=Plan::new(ctx,(approvals,target,payer),o.token_out,vec![],o.min_out,deadline)?;
    plan.set_direct_market(DirectCall::BancorV3(std::sync::Arc::new(proof)));
    Ok(plan)
}

pub(crate) fn probe(ctx:&Ctx,e:&Entry,s:&BancorState,o:&super::plan::Order,payer:Address,now:u64)
    ->GatewayResult<(Vec<approve::Approval>,Address,Bytes)>{
    let base=Form{module:true,native:false,exempt:false,desired:None};
    let t=terms(ctx,e,s,o.token_in,base).or_else(|_|terms(ctx,e,s,o.token_in,Form{module:false,..base}))?;
    let target=t.contract;
    let deadline=U256::from(now.checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||bad(ctx))?);
    let order=sim::Order{payer,target,input:o.token_in,output:o.token_out,amount:o.amount_in,minimum:o.min_out,wire:sim::Wire::BancorV3(Box::new(t.clone()))};
    Ok((approve::fork_exact(ctx,o.token_in,target,None,o.amount_in)?,target,t.proof(&order,o.min_out,deadline).data()))
}
