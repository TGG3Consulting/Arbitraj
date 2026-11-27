use super::{Quoting,routing::Input,v2_execution::{self,Execution,OptionQuote},v2_wallet_sim::{V5Step,Wire},white_state::Head};
use crate::{core::types::{RouteMode,Side},venues::bsc_dex::{exec::{aktionariat_plan,aktionariat_wallet,executor},
    pools::{family_variant::Variant,registry::Entry},num}};
use alloy_primitives::{Address,Bytes};

pub(super) async fn options(q:&Quoting,i:&Input,e:&Entry,payer:Address,graph:bool,head:Option<Head>)->Vec<OptionQuote> {
    if e.family.variant!=Variant::Brokerbot {return vec![];}
    let Ok(m)=aktionariat_plan::terms(&q.ctx,e) else{return vec![];};
    if let Some(h)=head && aktionariat_plan::verify(&q.ctx,e,m,&h.selector()).await.is_err(){return vec![];}
    let direct=v2_execution::option(q,i,e.clone(),payer,Execution{wire:Wire::Aktionariat(m),mode:RouteMode::Direct,target:m.module},head);
    let graph_option=async{
        if !graph{return None;}
        let exec=executor::address(&q.ctx).ok()??;
        let crate::core::types::PoolState::Brokerbot(s)=&e.pool.state else{return None;};
        let (input,buy)=if i.side==Side::Buy{(i.usdt,true)}else{(i.coin,false)};
        let qty=if buy{super::direct::ceil_units(i.qty,i.coin_dec)?}else{num::to_units(i.qty,i.coin_dec)?};
        let cap=s.swap(input,!buy,qty)?.amount_in;
        let id=aktionariat_wallet::step_id(m.broker,Some(cap))?;
        let step=V5Step{native:None,kind:17,pool:m.module,data:Bytes::copy_from_slice(id.as_slice())};
        let option=Execution{wire:Wire::AktionariatV5(step,m),mode:RouteMode::Graph,target:exec};
        v2_execution::option(q,i,e.clone(),payer,option,head).await.ok()
    };
    let (wallet,v5)=tokio::join!(direct,graph_option);
    wallet.ok().into_iter().chain(v5).collect()
}
