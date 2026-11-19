//! Файл: core/sell_here_ok.rs
//! Что делает: решение 417 (ревью t201, С-1 и М-2; правило Тиграна 19.11: «если продать на месте выгодно — итог
//! сделки ≥ 0 — другие площадки даже не искать, продаём здесь») — продажа с проверкой условия, по которому она решена,
//! на котировке ПОД ХОДОМ очереди продажи счёта (решение 415), по образцу sell.rs sell_or_stuck_guarded: котировка-
//! решение берётся без хода (sell_turn_fresh::once), и между ней и ордером другая сделка той же монеты на той же
//! площадке могла продать — поэтому перед ордером котировка под ходом и снова условие. Выполнено — уведомление и ордер
//! тем же ходом (sell.rs sell); не выполнено или котировки нет — ордера нет, ход снят, сделка возвращается вызывающему
//! с выручкой под ходом (он решает дальше по своим правилам).
//! «417 (ожидание)» (Тигран 19.11 через координатора, остаточный риск t201): то же для всех автоматических продаж
//! режима застрявшей монеты, решённых по котировке-решению: «Ожидание» (стало выгодно; по истечении срока при итоге
//! ≥ 0), «Порог убытка» (stuck.max_loss_pct), продажа здесь по stuck.relay_max_loss_pct (relay_here.rs), повтор после
//! «мало средств» с порогом режима (sell_fact.rs retry, Rule::MaxLoss). Условие — Cond; пере-замер, правило ветки 268
//! и уведомление — How.
//! Кто вызывает: core/relay_here.rs (try_sell, try_sell_if), core/stuck.rs (wait, loss_limit), core/sell_fact.rs (retry).
//! От чего зависит: sell_fact.rs (факт перед продажей), sell_turn.rs, sell_turn_fresh.rs, sell.rs (profitable, sell),
//! remeasure.rs, sell_range.rs (Rule), notify/human.rs, deal_card.rs.
//! Настройки: нет (базовое поведение; уведомление — событие stuck.market_sold, как у продажи режима).

use rust_decimal::Decimal;

use crate::notify::human::{self as h, Msg};

use super::deal::Deal;
use super::engine::Core;
use super::remeasure::Guard;
use super::sell_range::Rule;

/// Итог попытки продать с проверкой под ходом.
pub enum HereOk {
    /// Условие под ходом выполнено — ордер отправлен (дальше — обычный итог продажи sell.rs).
    Sold,
    /// Сделки больше нет у вызывающего (факт счёта: закрыта или алерт человеку — sell_fact::before).
    Gone,
    /// Ордера нет: условие под ходом не выполнено (Some — выручка под ходом) или котировки нет (None). Сделка — обратно.
    NotNow(Box<Deal>, Option<Decimal>),
}

/// Условие, по которому продажа решена.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    /// Итог сделки ≥ 0 (sell.rs profitable, решение 47).
    Profitable,
    /// Убыток, %, не выше порога (Deal::loss_pct_if ≤ порог; stuck.max_loss_pct / stuck.relay_max_loss_pct).
    MaxLoss(Decimal),
}

impl Cond {
    /// Выполнено ли условие при выручке v. Принимает: сделку, выручку. Ошибок нет.
    pub fn ok(&self, deal: &Deal, v: Decimal) -> bool {
        match self {
            Cond::Profitable => super::sell::profitable(deal, v),
            Cond::MaxLoss(m) => deal.loss_pct_if(v) <= *m,
        }
    }
}

/// Уведомление перед ордером.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// INFO «Продажа здесь: итог сделки не ниже нуля» (М-4).
    ProfitHere,
    /// WARN «Застрявшая монета продаётся по рынку» — как stuck::sold_by_market.
    StuckMarket,
    /// Без уведомления (как прежний вызов market_guarded без него).
    None,
}

/// Как продавать: условие, граница пере-замера DEX (remeasure), правило ветки 268 (sell_range.rs), уведомление.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct How {
    pub cond: Cond,
    pub guard: Guard,
    pub rule: Rule,
    pub note: Note,
}

/// Продажа здесь при итоге ≥ 0 (М-2, relay_here.rs): пере-замер «выгодно», правило Entry, уведомление INFO.
pub const PROFIT_HERE: How = How {
    cond: Cond::Profitable,
    guard: super::remeasure::PROFITABLE,
    rule: Rule::Entry,
    note: Note::ProfitHere,
};

/// Что делает: продать здесь, если под ходом очереди итог сделки ≥ 0 (PROFIT_HERE). Принимает: ядро, сделку,
/// причину. Возвращает: HereOk. Ошибок нет.
pub async fn try_sell(core: &Core, deal: Deal, why: &str) -> HereOk {
    Box::pin(try_sell_if(core, deal, why, PROFIT_HERE)).await
}

/// Что делает: продать, если под ходом очереди выполнено условие `how.cond` (шапка). Принимает: ядро, сделку, причину
/// (хронология, итог продажи), как продавать. Возвращает: HereOk. Ошибок нет.
pub async fn try_sell_if(core: &Core, deal: Deal, why: &str, how: How) -> HereOk {
    // Решение 413: на бирже — свежий факт и доля сделки перед продажей (как sell_or_stuck_guarded).
    let Some(deal) = Box::pin(super::sell_fact::before(core, deal, why)).await else {
        return HereOk::Gone;
    };
    // Решение 415: котировка и ордер — под одним ходом очереди продажи счёта.
    let mut turn = Box::pin(super::sell_turn::take(&deal.at, &deal.direction.asset)).await;
    match Box::pin(super::sell_turn_fresh::quote(core, &deal, &mut turn)).await {
        Ok((v, q, b)) if how.cond.ok(&deal, v) => {
            Box::pin(notify(core, &deal, why, how.note)).await;
            let f = super::remeasure::of(&deal, v, deal.qty * q.avg_price, how.guard);
            Box::pin(super::sell::sell(
                core,
                deal,
                (q, b, turn),
                why,
                f,
                how.rule,
            ))
            .await;
            HereOk::Sold
        }
        Ok((v, _, _)) => {
            drop(turn);
            HereOk::NotNow(Box::new(deal), Some(v))
        }
        Err(_) => {
            drop(turn);
            HereOk::NotNow(Box::new(deal), None)
        }
    }
}

/// Уведомление перед ордером (Note). Принимает: ядро, сделку, причину, вид. Ошибок нет.
async fn notify(core: &Core, deal: &Deal, why: &str, n: Note) {
    let (lvl, title) = match n {
        // М-4: продажа с итогом ≥ 0 — информативное уведомление, не «застрявшая монета».
        Note::ProfitHere => (h::INFO, "Продажа здесь: итог сделки не ниже нуля"),
        Note::StuckMarket => (h::WARN, "Застрявшая монета продаётся по рынку"),
        Note::None => return,
    };
    let text = Msg::new(lvl, title)
        .card(super::deal_card::card(deal))
        .reason(why)
        .id(&deal.id)
        .render();
    core.inner
        .deps
        .notify("stuck.market_sold", Some(&deal.at.0), &text)
        .await;
}
