//! Сравнение выплаты в ETH, обеспечения в WETH и атомарного шага V5.
use super::{Quoting,routing::Input,v2_execution::{self,Execution,OptionQuote},v2_wallet_sim::{self,Wire,V5Step},white_state::Head};
use super::super::{ctx::Ctx,exec::{executor,vista_wire::{Call,Form}},pools::{registry::Entry,surge}};
use crate::{core::types::{PoolState,RouteMode,surge::SurgeState},gateway::errors::{ErrorCode,GatewayResult}};
use alloy_primitives::{Address,Bytes,U256};

pub(crate) fn wire(ctx:&Ctx,e:&Entry,s:&SurgeState,input:Address,native:bool)->GatewayResult<(Address,Call)>{
    let buy=input==s.wrapped;
    if !buy && input!=s.token {return Err(ctx.err(ErrorCode::NoRoute,"Surge: неизвестный вход"));}
    let (target,form)=if e.family.executor_kind.is_some() && !native {
        (s.trader,Form::SurgeToken)
    } else if e.family.executor_kind.is_none() && native {
        (s.token,if buy {Form::SurgeEthIn}else{Form::SurgeEthOut})
    } else {return Err(ctx.err(ErrorCode::NoRoute,"Surge: выбранный вид расчёта требует другой физической валюты"))};
    Ok((target,Call{form,wrapped:s.wrapped,fee:U256::ZERO}))
}
pub(super) async fn options(q:&Quoting,i:&Input,entry:&Entry,payer:Address,input:Address,native:bool,h:Option<Head>)->Vec<OptionQuote>{
    let mut e=entry.clone();
    if let Some(head)=h{
        let Ok(state)=surge::market(&q.ctx,e.family,e.pool.reference.address(),(e.pool.token0,e.pool.token1),&head.selector()).await else {return vec![]};
        e.pool.state=PoolState::Surge(Box::new(state));e.pool.block=head.number.to();
    }
    let PoolState::Surge(s)=&e.pool.state else{return vec![]};
    let mut options=Vec::new();
    if let Ok((target,c))=wire(&q.ctx,&e,s,input,native) &&
        let Ok(result)=v2_execution::option(q,i,e.clone(),payer,Execution{mode:RouteMode::Direct,target,wire:Wire::Vista(c)},h).await {
        options.push(result);
    }
    if e.family.executor_kind.is_some() && !crate::gateway::quote_stage::settlement() &&
       !super::vista_pricing::cash(&q.ctx,&i.usdt_ticker,i.usdt) &&
       (!native||executor::native_capable(&q.ctx)) && let Ok(Some(target))=executor::address(&q.ctx){
        let v=V5Step{native:native.then_some((s.wrapped,input==s.wrapped)),kind:29,pool:s.trader,data:Bytes::new()};
        if let Ok(r)=v2_execution::option(q,i,e,payer,Execution{mode:RouteMode::Graph,target,wire:Wire::V5(v)},h).await {options.push(r)}
    }
    options
}

/// Встроенные getTokenAmountOut и getBNBAmountOut имеют иной порядок округления и не применяют налог.
/// На финальной сверке измеряем собственный перевод на той же высоте, включая собственный allowance.
pub(crate) async fn observed(ctx:&Ctx,e:&Entry,input:Address,amount:U256,block:&str)->GatewayResult<U256>{
    let h=super::white_state::head(ctx,block).await?;
    let s=surge::market(ctx,e.family,e.pool.reference.address(),(e.pool.token0,e.pool.token1),&h.selector()).await?;
    let payer=super::super::wallet::signer::address(ctx)?;
    let output=if input==s.wrapped {s.token}else{s.wrapped};
    let (target,c)=wire(ctx,e,&s,input,e.family.executor_kind.is_none())?;
    let o=v2_wallet_sim::Order{payer,target,input,output,amount,minimum:U256::ONE,wire:Wire::Vista(c)};
    Ok(Box::pin(v2_wallet_sim::measure(ctx,h,o,true)).await?.received)
}
