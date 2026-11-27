//! Compare actual settlement in the owner's requested currency, on the same pair of physical V1 reserves.
use super::{Quoting,routing::Input,white_state::Head,v2_execution::{self,Execution,OptionQuote},v2_wallet_sim::{V5Step,Wire}};
use crate::{core::types::{PoolState,RouteMode,Side},venues::bsc_dex::{
    exec::{executor,uniswap_v1_plan},pools::{registry::Entry,uniswap_v1}
}};
use alloy_primitives::{Address,Bytes};

pub(super) async fn options(q:&Quoting,i:&Input,e:&Entry,payer:Address,native:bool,graph:bool,head:Option<Head>)->Vec<OptionQuote>{
    let mut quotes=Vec::new();
    let ctx=&q.ctx;
    let src=if i.side==Side::Buy{i.usdt}else{i.coin};
    let mut entry=e.clone();
    if let Some(h)=head{
        let Ok(state)=uniswap_v1::load(ctx,e.family,&e.pool.reference,[e.pool.token0,e.pool.token1],&h.selector()).await else{return quotes;};
        entry.pool.state=PoolState::UniswapV1(Box::new(state));
    }
    let PoolState::UniswapV1(state)=&entry.pool.state else{return quotes;};
    for module in [false,true]{
        let Ok(t)=uniswap_v1_plan::terms(ctx,&entry,state,src,module,native) else{continue;};
        if let Some(h)=head && uniswap_v1_plan::check(ctx,&entry,state,&t,src,&h.selector()).await.is_err(){continue;}
        let target=t.target;
        let direct=Execution{mode:RouteMode::Direct,target,wire:Wire::UniswapV1(Box::new(t))};
        if let Ok(v)=v2_execution::option(q,i,entry.clone(),payer,direct,head).await{quotes.push(v);}
        if module && graph && let Ok(Some(v5))=executor::address(ctx){
            let key=entry.pool.reference.address().into_word();
            let via=Execution{mode:RouteMode::Graph,target:v5,wire:Wire::V5(V5Step{native:None,kind:17,pool:target,data:Bytes::copy_from_slice(key.as_slice())})};
            if let Ok(v)=v2_execution::option(q,i,entry.clone(),payer,via,head).await{quotes.push(v);}
        }
    }
    quotes
}
