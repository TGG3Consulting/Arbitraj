//! Compare BPool exact-in, exact-out and V5 on the same pool state and payer.
use super::{Quoting, routing::Input, v2_execution::{self,Execution,OptionQuote},
    v2_wallet_sim::{V5Step,Wire},white_state::Head};
use crate::{core::types::{PoolState,RouteMode,Side},
    venues::bsc_dex::{exec::{balancer_v1_plan::{self,Selection},executor},pools::{balancer_v1,registry::Entry}}};
use alloy_primitives::{Address,Bytes};

pub(super) async fn options(q:&Quoting,i:&Input,e:&Entry,payer:Address,native:bool,graph:bool,head:Option<Head>)
    ->Vec<OptionQuote>{
    let ctx=&q.ctx;
    let Ok(target)=ctx.dex_addr("dex.router",e.family.id) else{return vec![]};
    let pick=Selection{target,native,desired:None};
    let Ok(t)=balancer_v1_plan::terms(ctx,e,pick) else{return vec![]};
    let mut entry=e.clone();
    if let Some(h)=head{
        let at=h.selector();
        let Ok((state,()))=tokio::try_join!(balancer_v1::load(ctx,e.family,t.pool,&at),balancer_v1_plan::check(ctx,e,&t,&at))
            else{return vec![]};
        entry.pool.state=PoolState::BalancerV1(Box::new(state));
    }
    let make=|wire,mode,to| Execution{wire,mode,target:to};
    let source=v2_execution::option(q,i,entry.clone(),payer,make(Wire::BalancerV1(t),RouteMode::Direct,target),head);
    let by_target=async {
        if i.side!=Side::Buy{return None;}
        let want=super::direct::ceil_units(i.qty,i.coin_dec)?;
        let call=Wire::BalancerV1(crate::venues::bsc_dex::exec::balancer_v1_wallet::Terms{desired:Some(want),..t});
        v2_execution::option(q,i,entry.clone(),payer,make(call,RouteMode::Direct,target),head).await.ok()
    };
    let via=async {
        if !graph || native{return None;}
        let exec=executor::address(ctx).ok()??;
        let step=V5Step{native:None,kind:17,pool:target,data:Bytes::copy_from_slice(t.pool.into_word().as_slice())};
        v2_execution::option(q,i,entry.clone(),payer,make(Wire::V5(step),RouteMode::Graph,exec),head).await.ok()
    };
    let (direct,exact,step)=tokio::join!(source,by_target,via);
    direct.ok().into_iter().chain(exact).chain(step).collect()
}
