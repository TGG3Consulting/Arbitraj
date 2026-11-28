//! Выбор между обычным DEX и конкурентной персональной ставкой в пределах срока обеих ног.
use super::*;
use crate::{gateway::errors::{ErrorCode,GatewayResult},venues::bsc_dex::hashflow};
use futures_util::FutureExt;
use std::{future::Future,time::Duration};
use tokio::time::Instant;

/// Обычная цепочка выбирает маршрут одновременно с личной котировкой. Таймер включает всю работу RFQ.
pub(in crate::venues::bsc_dex) async fn compete(
    p:&Parts,asset:&Asset,side:Side,qty:Decimal,ordinary:impl Future<Output=GatewayResult<Quote>>,
)->GatewayResult<Quote>{
    if !crate::venues::bsc_dex::tokenlon::enabled(&p.ctx) || !crate::venues::bsc_dex::tokenlon::private_node::configured(&p.ctx) || qty<=Decimal::ZERO || quote_stage::settlement() {
        return ordinary.await;
    }
    let Ok(ms)=p.ctx.int("tokenlon.quote_budget_ms") else{return ordinary.await};
    let budget=Duration::from_millis(ms);
    let Some(end)=Instant::now().checked_add(budget) else{return ordinary.await};
    let indicative=Box::pin(candidate(p,asset,side,qty,false)).await;
    if !quote_stage::is_final() {
        let old=ordinary.await;
        return choose(p,side,qty,old,indicative).await;
    }
    if !available(p,asset,side) {return ordinary.await;}
    let mut ordinary=std::pin::pin!(ordinary);
    let firm=tokio::time::timeout_at(end,Box::pin(candidate(p,asset,side,qty,true)));
    let mut firm=std::pin::pin!(firm);
    let old=tokio::select! {
        biased;
        old=&mut ordinary=>old,
        ready=&mut firm=>return choose(p,side,qty,ordinary.await,ready.ok().flatten()).await,
    };
    if old.as_ref().is_err_and(|e|e.code!=ErrorCode::NoRoute) {return old;}
    let available=old.as_ref().ok().filter(|q|complete(q,qty));
    // Готовый обычный маршрут ждёт только конкурентоспособную измеренную ставку.
    let wait=if let Some(q)=available {
        let promising=if let Some((off,_))=&indicative {
            better(p,side,off,q).await
        } else {false};
        if promising {fallback_deadline(end,budget,quote_stage::calc_deadline())} else {None}
    } else {Some(end)};
    let ready=until_ready(wait,&mut firm).await;
    choose(p,side,qty,old,ready.and_then(Result::ok).flatten()).await
}

/// Оставляем один настроенный бюджет следующей ноге и проверкам. Поздний вход принимает лишь готовую firm.
fn fallback_deadline(end:Instant,budget:Duration,outer:Option<Instant>)->Option<Instant>{
    match outer {Some(t)=>t.checked_sub(budget).map(|t|end.min(t)),None=>Some(end)}
}

fn complete(q:&Quote,qty:Decimal)->bool {
    q.qty==qty && q.fillable_qty==qty && (!quote_stage::is_final() || q.route.as_ref().is_none_or(|r|r.executable))
}

/// Обе оценки включают газ из текущих пулов и затраты вне цены. pinned decimals исключают RPC.
async fn better(p:&Parts,side:Side,offer:&Quote,old:&Quote)->bool {
    let Some(a)=hashflow::routing::total(p,offer).await else{return false};
    let Some(b)=hashflow::routing::total(p,old).await else{return false};
    if side==Side::Buy {a<b} else {a>b}
}

async fn choose(p:&Parts,side:Side,qty:Decimal,old:GatewayResult<Quote>,ready:Option<Offer>)->GatewayResult<Quote>{
    let Some((q,ticket))=ready else{return old};
    if old.as_ref().is_err_and(|e|e.code!=ErrorCode::NoRoute) {return old;}
    if let Some(other)=old.as_ref().ok().filter(|q|complete(q,qty)) {
        if !better(p,side,&q,other).await {return old;}
    } else if hashflow::routing::total(p,&q).await.is_none(){return old;}
    if let Some(t)=ticket {
        if p.ctx.settings.generation()!=t.revision || !crate::venues::bsc_dex::tokenlon::timely(&p.ctx,t.proof.expiry) {return old;}
        p.tokenlon.save(t);
    }
    Ok(q)
}

/// На истёкшей границе один опрос забирает уже готовую котировку без ожидания сети.
async fn until_ready<F:Future>(deadline:Option<Instant>,work:F)->Option<F::Output> {
    match deadline.filter(|t|*t>Instant::now()) {
        Some(t)=>tokio::time::timeout_at(t,work).await.ok(),
        None=>work.now_or_never(),
    }
}


#[cfg(test)]
#[path="competition_tests.rs"]
mod competition_tests;
