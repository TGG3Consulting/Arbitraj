//! Файл: core/sell.rs
//! Что делает: продажа монет сделки там, где они сейчас (шаг 5 ТЗ). Выгодно — итог сделки при продаже
//! сейчас не ниже нуля (решение 47: любая прибыль забирается сразу; порог прибыли решает только вход) —
//! продаём; иначе — режим застрявшей монеты (stuck.rs, решение 37).
//! Продажа по рынку — для режимов «Ожидание», «Порог убытка», «Перегон» и кнопки «Продать сейчас».
//! Продано всё — сделка закрыта, возврат USDT в каждой сделке (usdt_return.rs); продано не всё —
//! остаток снова через режим застрявшей монеты (меньше шага или минимума ордера биржи — пыль, dust.rs, решение 66); продажа не прошла — сделка остановлена, монеты на месте.
//! Кто вызывает: executor.rs, stuck.rs, manual.rs.
//! От чего зависит: trade_ops.rs, fills.rs, ledger.rs, stuck.rs, usdt_return.rs, deal.rs.
//! Настройки: нет своих.

use rust_decimal::Decimal;

use crate::gateway::errors::ErrorCode;

use crate::core::types::{Quote, Side};

use super::deal::{Deal, Step};
use super::engine::Core;

/// Выгодно ли продать за value (USDT чистыми): итог сделки не ниже нуля (решение 47).
pub fn profitable(deal: &Deal, value: Decimal) -> bool {
    deal.result_if(value) >= Decimal::ZERO
}

/// Продать сейчас, если выгодно, иначе — режим застрявшей монеты. Ошибок нет.
pub async fn sell_or_stuck(core: &Core, deal: Deal) {
    let value = super::trade_ops::sell_value(core, &deal.at, &deal.direction.asset, deal.qty).await;
    match value {
        Ok((v, q)) if profitable(&deal, v) => sell(core, deal, q, "выгодно").await,
        _ => Box::pin(super::stuck::handle(core, deal)).await,
    }
}

/// Продажа по рынку (по текущей котировке; худшая цена — trade_ops). Ошибок нет: не вышло —
/// сделка остановлена, монеты остаются на площадке (видно на экране «Сделки»).
pub async fn market(core: &Core, deal: Deal, why: &str) {
    match super::trade_ops::sell_value(core, &deal.at, &deal.direction.asset, deal.qty).await {
        Ok((_, q)) => sell(core, deal, q, why).await,
        Err(e) => {
            let mut deal = deal;
            super::ledger::finish(
                core,
                &mut deal,
                Step::Halted,
                &format!("продажа по рынку ({why}) невозможна: {e}"),
            )
            .await;
        }
    }
}

/// Продажа всего количества по котировке q. Принимает: сделку, котировку, причину (для хронологии).
async fn sell(core: &Core, mut deal: Deal, q: Quote, why: &str) {
    let (v, a) = (deal.at.clone(), deal.direction.asset.clone());
    match super::trade_ops::trade(
        core,
        &deal.id,
        &v,
        &a,
        Side::Sell,
        deal.qty,
        q.route.clone(),
        q.avg_price,
        None,
    )
    .await
    {
        Ok(r) => {
            super::fills::trade(core, &mut deal, &r).await;
            deal.sells.push(r.clone());
            let text = format!(
                "{}: продано {} {} на {} по {} ({why})",
                deal.id, r.filled_qty, a.ticker, v.0, r.avg_price
            );
            core.inner
                .deps
                .notify("deal.sell_filled", Some(&v.0), &text)
                .await;
            // Решение 66: остаток меньше шага или минимума ордера — пыль, а не застрявшая монета.
            super::dust::settle(core, &mut deal, r.avg_price).await;
            if deal.qty > Decimal::ZERO {
                deal.note(format!("Продано не всё, остаток {}", deal.qty));
                super::ledger::save(core, &deal).await;
                return Box::pin(super::stuck::handle(core, deal)).await;
            }
            super::ledger::finish(core, &mut deal, Step::Closed, &format!("продано ({why})")).await;
            super::usdt_return::after_deal(core, &deal).await;
        }
        // BNB на DEX (решение 62): отказ резерва на газ (InsufficientBalance) — монета на месте, просто газ продажи
        // сейчас не помещается сверх резерва: «застрявшая монета» (stuck.rs), а не остановка сделки.
        Err(e)
            if e.code == Some(ErrorCode::InsufficientBalance)
                && a.ticker == super::convert::GAS_TICKER =>
        {
            deal.note(format!("продажа не прошла: {e} — застрявшая монета"));
            super::ledger::save(core, &deal).await;
            Box::pin(super::stuck::handle(core, deal)).await
        }
        Err(e) => {
            super::ledger::finish(
                core,
                &mut deal,
                Step::Halted,
                &format!("продажа не прошла: {e}"),
            )
            .await;
        }
    }
}
