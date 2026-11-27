use super::{chain::Step, plan::{DirectCall,Plan}, uniswap_v1_wallet::{IV1Wallet,Terms}};
use crate::{
    core::types::uniswap_v1::UniswapV1State,
    gateway::errors::{ErrorCode,GatewayResult},
    venues::bsc_dex::{ctx::Ctx,pools::{dagrow,direct_module,exact_wallet_tokens,registry::Entry,uniswap_v1},
        quote::{v2_wallet_sim as sim,white_state},wallet::approve}
};
use alloy_primitives::{Address,Bytes,U256};
use serde_json::Value;

fn bad(ctx:&Ctx)->crate::gateway::errors::GatewayError{
    ctx.err(ErrorCode::NoRoute,"Uniswap V1: форма обмена, рынок или получатель не совпадают с котировкой")
}

pub(crate) fn terms(ctx:&Ctx,e:&Entry,s:&UniswapV1State,src:Address,module:bool,native:bool)->GatewayResult<Terms>{
    let id=e.family.id;
    let side=s.side(src).ok_or_else(||bad(ctx))?;
    let (input_pool,output_pool)=(s.markets[side].as_ref().map(|m|m.address),s.markets[1-side].as_ref().map(|m|m.address));
    let first=input_pool.or(output_pool).ok_or_else(||bad(ctx))?;
    let wrapped=ctx.addr("dex.native_wrapped")?;
    let exchange_hash=ctx.dex_text("dex.pair_runtime_hash",id)?.parse().map_err(|_|bad(ctx))?;
    let (target,module_hash)=if module {
        if native && s.dual(){return Err(bad(ctx));}
        let target=ctx.dex_addr("dex.router",id)?;
        let hashes=direct_module::configured(ctx,id)?;
        let [pin]=hashes.as_slice() else{return Err(bad(ctx));};
        if target.is_zero() || [input_pool,output_pool].contains(&Some(target)){return Err(bad(ctx));}
        (target,Some(*pin))
    }else{
        if native==s.dual() || ![input_pool,output_pool].into_iter().zip([src,s.tokens[1-side]])
            .all(|(exchange,token)|exchange.is_none() || exact_wallet_tokens::eligible_input(ctx,id,token)){return Err(bad(ctx));}
        (first,None)
    };
    Ok(Terms{target,wrapped,input_pool,output_pool,exchange_hash,module_hash,physical_module:module && native})
}

pub(crate) async fn check(ctx:&Ctx,e:&Entry,s:&UniswapV1State,t:&Terms,src:Address,at:&Value)->GatewayResult<()>{
    if t.module_hash.is_some(){
        use crate::venues::bsc_dex::node::call::canonical;
        let ((),factory,wrapped)=tokio::try_join!(direct_module::verify_code(ctx,e.family.id,t.target,at),
            canonical(ctx,at,t.target,IV1Wallet::factoryCall{}),canonical(ctx,at,t.target,IV1Wallet::wrappedCall{}))?;
        if factory!=s.factory || wrapped!=t.wrapped{return Err(bad(ctx));}
    }else{
        let side=s.side(src).ok_or_else(||bad(ctx))?;
        if t.input_pool.is_some(){exact_wallet_tokens::check_input(ctx,e.family.id,src,at).await?;}
        if t.output_pool.is_some(){exact_wallet_tokens::check_input(ctx,e.family.id,s.tokens[1-side],at).await?;}
    }
    Ok(())
}

pub(super) async fn plan(ctx:&Ctx,step:&Step,payer:Address,height:u64,selection:Option<(Address,bool,bool)>)->GatewayResult<Plan>{
    let generation=ctx.settings.generation();
    let (target,module,native)=selection.ok_or_else(||bad(ctx))?;
    if !step.from_wallet || !step.to_wallet{return Err(bad(ctx));}
    let h=white_state::head(ctx,&format!("{height:#x}")).await?;
    let at=h.selector();
    let e=&step.entry;
    let o=&step.order;
    let s=uniswap_v1::load(ctx,e.family,&e.pool.reference,[e.pool.token0,e.pool.token1],&at).await?;
    let t=terms(ctx,e,&s,o.token_in,module,native)?;
    if t.target!=target{return Err(bad(ctx));}
    check(ctx,e,&s,&t,o.token_in,&at).await?;
    let order=sim::Order{payer,target,input:o.token_in,output:o.token_out,amount:o.amount_in,minimum:o.min_out,wire:sim::Wire::UniswapV1(Box::new(t.clone()))};
    let deadline=U256::from(h.timestamp.to::<u64>().checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||bad(ctx))?);
    let proof=t.proof(&order,o.min_out,deadline);
    let approvals=if t.value(o.amount_in).is_zero(){
        let current=dagrow::allowance(ctx,o.token_in,payer,target,&at).await?;
        approve::fork_exact(ctx,o.token_in,target,Some(current),o.amount_in)?
    }else{vec![]};
    sim::measure(ctx,h,order,true).await?;
    if ctx.settings.generation()!=generation{return Err(bad(ctx));}
    let mut built=Plan::new(ctx,(approvals,target,payer),o.token_out,vec![],o.min_out,deadline)?;
    built.set_direct_market(DirectCall::UniswapV1(std::sync::Arc::new(proof)));
    Ok(built)
}

pub(crate) fn probe(ctx:&Ctx,e:&Entry,s:&UniswapV1State,o:&super::plan::Order,payer:Address,now:u64)
    ->GatewayResult<(Vec<approve::Approval>,Address,Bytes)>{
    // ERC20 round-trip probes use the protected wrapper, or the original two-market transfer.
    let t=terms(ctx,e,s,o.token_in,true,false).or_else(|_|terms(ctx,e,s,o.token_in,false,false))?;
    let target=t.target;
    let deadline=U256::from(now.checked_add(ctx.int("dex.deadline_s")?).ok_or_else(||bad(ctx))?);
    let order=sim::Order{payer,target,input:o.token_in,output:o.token_out,amount:o.amount_in,minimum:o.min_out,wire:sim::Wire::UniswapV1(Box::new(t.clone()))};
    Ok((approve::fork_exact(ctx,o.token_in,target,None,o.amount_in)?,target,t.proof(&order,o.min_out,deadline).data()))
}
