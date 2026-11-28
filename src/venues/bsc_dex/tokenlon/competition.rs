//! Выбор между обычным DEX и конкурентной персональной ставкой в пределах срока обеих ног.
use super::*;
use crate::venues::bsc_dex::tokenlon;
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::hashflow,
};
use futures_util::FutureExt;
use std::{future::Future, time::Duration};
use tokio::time::Instant;

/// Обычная цепочка выбирает маршрут одновременно с личной котировкой. Таймер включает всю работу RFQ.
pub(in crate::venues::bsc_dex) async fn compete(
    p: &Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    ordinary: impl Future<Output = GatewayResult<Quote>>,
) -> GatewayResult<Quote> {
    if !tokenlon::enabled(&p.ctx)
        || !tokenlon::private_node::configured(&p.ctx)
        || qty <= Decimal::ZERO
        || quote_stage::settlement()
    {
        return ordinary.await;
    }
    let Ok(ms) = p.ctx.int("tokenlon.quote_budget_ms") else {
        return ordinary.await;
    };
    let budget = Duration::from_millis(ms);
    let Some(end) = Instant::now().checked_add(budget) else {
        return ordinary.await;
    };
    if !quote_stage::is_final() {
        let old = ordinary.await;
        if protected(&old) {
            return old;
        }
        return choose(p, side, qty, old, indicative(p, asset, side, qty));
    }
    if !available(p, asset, side) {
        return ordinary.await;
    }
    let firm = Box::pin(candidate(p, asset, side, qty, true));
    let (old, ready) = competition(ordinary, firm, end, |old| {
        if protected(old) {
            return Window::Veto;
        }
        let Some(q) = old.as_ref().ok().filter(|q| complete(q, qty)) else {
            return Window::Until(end);
        };
        let promising =
            indicative(p, asset, side, qty).is_some_and(|(off, _)| better(p, side, &off, q));
        if !promising {
            return Window::Ready;
        }
        fallback_deadline(end, budget, quote_stage::calc_deadline())
            .map_or(Window::Ready, Window::Until)
    })
    .await;
    choose(p, side, qty, old, ready)
}

enum Window {
    Veto,
    Ready,
    Until(Instant),
}

/// Когда обычный маршрут готов, защитный отказ прекращает гонку. Опоздавшая симуляция не участвует.
async fn competition<A, B>(
    ordinary: impl Future<Output = A>,
    firm: impl Future<Output = Option<B>>,
    end: Instant,
    window: impl FnOnce(&A) -> Window,
) -> (A, Option<B>) {
    let mut ordinary = std::pin::pin!(ordinary);
    let timed = tokio::time::timeout_at(end, async {
        if Instant::now() >= end {
            return None;
        }
        let offer = firm.await;
        offer.filter(|_| Instant::now() <= end)
    });
    let mut firm = std::pin::pin!(timed);
    let old = tokio::select! {
        biased;
        old=&mut ordinary=>old,
        ready=&mut firm=>return (ordinary.await,ready.ok().flatten()),
    };
    let ready = match window(&old) {
        Window::Veto => None,
        Window::Ready => until_ready(None, &mut firm).await,
        Window::Until(t) => until_ready(Some(t), &mut firm).await,
    };
    (old, ready.and_then(Result::ok).flatten())
}

fn protected(old: &GatewayResult<Quote>) -> bool {
    old.as_ref().is_err_and(|e| e.code != ErrorCode::NoRoute)
}

/// Эти пути используют только локальную книгу и pinned decimals. Любое ожидание оставляет ordinary.
fn indicative(p: &Parts, asset: &Asset, side: Side, qty: Decimal) -> Option<Offer> {
    Box::pin(candidate(p, asset, side, qty, false))
        .now_or_never()
        .flatten()
}

fn total(p: &Parts, q: &Quote) -> Option<Decimal> {
    hashflow::routing::total(p, q).now_or_never().flatten()
}

/// Оставляем один настроенный бюджет следующей ноге и проверкам. Поздний вход принимает лишь готовую firm.
fn fallback_deadline(end: Instant, budget: Duration, outer: Option<Instant>) -> Option<Instant> {
    match outer {
        Some(t) => t.checked_sub(budget).map(|t| end.min(t)),
        None => Some(end),
    }
}

fn complete(q: &Quote, qty: Decimal) -> bool {
    q.qty == qty
        && q.fillable_qty == qty
        && (!quote_stage::is_final() || q.route.as_ref().is_none_or(|r| r.executable))
}

/// Обе оценки включают газ из текущих пулов и затраты вне цены. pinned decimals исключают RPC.
fn better(p: &Parts, side: Side, offer: &Quote, old: &Quote) -> bool {
    let Some(a) = total(p, offer) else {
        return false;
    };
    let Some(b) = total(p, old) else { return false };
    if side == Side::Buy { a < b } else { a > b }
}

fn choose(
    p: &Parts,
    side: Side,
    qty: Decimal,
    old: GatewayResult<Quote>,
    ready: Option<Offer>,
) -> GatewayResult<Quote> {
    let Some((q, ticket)) = ready else { return old };
    if protected(&old) {
        return old;
    }
    if let Some(other) = old.as_ref().ok().filter(|q| complete(q, qty)) {
        if !better(p, side, &q, other) {
            return old;
        }
    } else if total(p, &q).is_none() {
        return old;
    }
    if let Some(t) = ticket {
        if p.ctx.settings.generation() != t.revision || !tokenlon::timely(&p.ctx, t.proof.expiry) {
            return old;
        }
        p.tokenlon.save(t);
    }
    Ok(q)
}

/// На истёкшей границе один опрос забирает уже готовую котировку без ожидания сети.
async fn until_ready<F: Future>(deadline: Option<Instant>, work: F) -> Option<F::Output> {
    match deadline.filter(|t| *t > Instant::now()) {
        Some(t) => tokio::time::timeout_at(t, work).await.ok(),
        None => work.now_or_never(),
    }
}

#[cfg(test)]
#[path = "competition_tests.rs"]
mod competition_tests;
