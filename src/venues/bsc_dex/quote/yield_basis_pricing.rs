use super::{Quoting,routing::Input,v2_execution::{self,Execution,OptionQuote},v2_wallet_sim::{V5Step,Wire},white_state::Head};
use crate::{core::types::RouteMode,venues::bsc_dex::{exec::{executor,yield_basis_plan},pools::{registry::Entry,yield_basis_quotes}}};
use alloy_primitives::{Address,Bytes};

pub(super) async fn options(q:&Quoting,i:&Input,e:&Entry,payer:Address,native:bool,graph:bool,head:Option<Head>)->Vec<OptionQuote> {
    let ctx=&q.ctx;
    let Ok(target)=ctx.dex_addr("dex.router",e.family.id) else{return vec![];};
    let Ok(t)=yield_basis_plan::terms(ctx,e,target,native) else{return vec![];};
    let mut entry=e.clone();
    if let Some(h)=head {
        let at=h.selector();
        let Ok((state,()))=tokio::try_join!(yield_basis_quotes::load(ctx,&q.tokens,&q.reg,e.family,&e.pool.reference,(e.pool.token0,e.pool.token1),e.usdt,&at),
                                             yield_basis_plan::check_module(ctx,e,t,&at)) else{return vec![];};
        entry.pool.state=state;
    }
    let direct=v2_execution::option(q,i,entry.clone(),payer,Execution{wire:Wire::YieldBasis(t),mode:RouteMode::Direct,target},head);
    let graph_option=async {
        if !graph || native{return None;}
        let exec=executor::address(ctx).ok()??;
        let step=V5Step{native:None,kind:17,pool:target,data:Bytes::copy_from_slice(t.market.as_slice())};
        v2_execution::option(q,i,entry,payer,Execution{wire:Wire::V5(step),mode:RouteMode::Graph,target:exec},head).await.ok()
    };
    let (wallet,step)=tokio::join!(direct,graph_option);
    wallet.ok().into_iter().chain(step).collect()
}
