//! Файл: core/sell_here_ok.rs
//! Что делает: решение 417 (ревью t201, С-1 и М-2; правило Тиграна 19.11: «если продать на месте выгодно — итог
//! сделки ≥ 0 — другие площадки даже не искать, продаём здесь») — продажа здесь при итоге ≥ 0 с проверкой условия
//! «выгодно» на котировке ПОД ХОДОМ очереди продажи счёта (решение 415), по образцу sell.rs sell_or_stuck_guarded:
//! котировка-решение берётся без хода (sell_turn_fresh::once), и между ней и ордером другая сделка той же монеты на
//! той же площадке могла продать — поэтому перед ордером котировка под ходом и снова profitable. Под ходом итог ≥ 0 —
//! уведомление (информативное, не «застрявшая монета» — М-4) и ордер тем же ходом (sell.rs sell, пере-замер DEX по
//! условию «выгодно» remeasure::PROFITABLE); итог < 0 или котировки нет — ордера нет, ход снят, сделка возвращается
//! вызывающему (продажа здесь по порогу stuck.relay_max_loss_pct — relay_here.rs).
//! Кто вызывает: core/relay_here.rs (sell_here_as — все пути продажи здесь режима «Перегон»: перегон не выгоднее,
//! перегоны исчерпаны, пауза, вывод отклонён, остаток, executor.rs, sell_unsent::here, transfer_failed; relay.rs
//! правило 1 — через sell_here).
//! От чего зависит: sell_fact.rs (факт перед продажей), sell_turn.rs, sell_turn_fresh.rs, sell.rs (profitable, sell),
//! remeasure.rs, sell_range.rs (Rule), notify/human.rs, deal_card.rs.
//! Настройки: нет (базовое поведение; уведомление — событие stuck.market_sold, как у продажи режима).

use rust_decimal::Decimal;

use crate::notify::human::{self as h, Msg};

use super::deal::Deal;
use super::engine::Core;
use super::sell_range::Rule;

/// Итог попытки продать здесь при итоге ≥ 0.
pub enum HereOk {
    /// Под ходом итог ≥ 0 — ордер отправлен (дальше — обычный итог продажи sell.rs).
    Sold,
    /// Сделки больше нет у вызывающего (факт счёта: закрыта или алерт человеку — sell_fact::before).
    Gone,
    /// Ордера нет: под ходом итог < 0 (Some — выручка под ходом) или котировки нет (None). Сделка — обратно.
    NotNow(Box<Deal>, Option<Decimal>),
}

/// Что делает: продать здесь, если под ходом очереди итог сделки ≥ 0 (шапка). Принимает: ядро, сделку, причину
/// (хронология, итог продажи). Возвращает: HereOk. Ошибок нет.
pub async fn try_sell(core: &Core, deal: Deal, why: &str) -> HereOk {
    // Решение 413: на бирже — свежий факт и доля сделки перед продажей (как sell_or_stuck_guarded).
    let Some(deal) = Box::pin(super::sell_fact::before(core, deal, why)).await else {
        return HereOk::Gone;
    };
    // Решение 415: котировка и ордер — под одним ходом очереди продажи счёта.
    let mut turn = Box::pin(super::sell_turn::take(&deal.at, &deal.direction.asset)).await;
    match Box::pin(super::sell_turn_fresh::quote(core, &deal, &mut turn)).await {
        Ok((v, q, b)) if super::sell::profitable(&deal, v) => {
            // М-4: продажа с итогом ≥ 0 — информативное уведомление, не «застрявшая монета».
            let text = Msg::new(h::INFO, "Продажа здесь: итог сделки не ниже нуля")
                .card(super::deal_card::card(&deal))
                .reason(why)
                .id(&deal.id)
                .render();
            core.inner
                .deps
                .notify("stuck.market_sold", Some(&deal.at.0), &text)
                .await;
            let g = super::remeasure::PROFITABLE;
            let f = super::remeasure::of(&deal, v, deal.qty * q.avg_price, g);
            Box::pin(super::sell::sell(
                core,
                deal,
                (q, b, turn),
                why,
                f,
                Rule::Entry,
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
