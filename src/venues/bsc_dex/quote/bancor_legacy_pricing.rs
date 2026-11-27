use super::{Quoting,routing::Input,v2_execution::{self,Execution,OptionQuote},v2_wallet_sim::{V5Step,Wire},white_state::Head};
use crate::{core::types::{RouteMode,Side},venues::bsc_dex::{exec::{executor,bancor_legacy_plan},pools::registry::Entry}};
use alloy_primitives::{Address,Bytes};

pub(super) async fn options(q:&Quoting,i:&Input,e:&Entry,payer:Address,native:bool,graph:bool,head:Option<Head>)->Vec<OptionQuote> {
    let ctx=&q.ctx;
    let Ok(target)=ctx.dex_addr("dex.router",e.family.id) else{return vec![];};
    let Ok(terms)=bancor_legacy_plan::terms(ctx,e,target,native) else{return vec![];};
    let input=if i.side==Side::Buy{i.usdt}else{i.coin};
    if let Some(h)=head && bancor_legacy_plan::verify(ctx,e,input,terms,&h.selector()).await.is_err(){return vec![];}
    let direct=v2_execution::option(q,i,e.clone(),payer,Execution{wire:Wire::BancorLegacy(Box::new(terms)),mode:RouteMode::Direct,target},head);
    let v5=async {
        if !graph || native{return None;}
        let exec=executor::address(ctx).ok()??;
        let key=bancor_legacy_plan::step_id(e,input)?;
        let step=V5Step{native:None,kind:17,pool:target,data:Bytes::copy_from_slice(key.as_slice())};
        v2_execution::option(q,i,e.clone(),payer,Execution{wire:Wire::V5(step),mode:RouteMode::Graph,target:exec},head).await.ok()
    };
    let (wallet,step)=tokio::join!(direct,v5);
    wallet.ok().into_iter().chain(step).collect()
}
