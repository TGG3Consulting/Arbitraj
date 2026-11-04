//! Файл: core/sell.rs
//! Что делает: продажа монет сделки там, где они сейчас (шаг 5 ТЗ). Выгодно — итог сделки при продаже
//! сейчас не ниже нуля (решение 47: любая прибыль забирается сразу; порог прибыли решает только вход) —
//! продаём; иначе — режим застрявшей монеты (stuck.rs, решение 37).
//! Продажа по рынку — для режимов «Ожидание», «Порог убытка», «Перегон» и кнопки «Продать сейчас».
//! Продано всё — сделка закрыта (возврат USDT — общий шаг закрытия ledger::finish, решение 138); продано не всё —
//! остаток снова через режим застрявшей монеты (меньше шага или минимума ордера биржи — пыль, dust.rs, решение 66); продажа не прошла — алерт человеку
//! (manual.rs, решение 100), монеты на месте; продажа не ушла в сеть (решение 110; ни один канал не принял, и узел
//! подтвердил, что транзакции нет, — решение 154) — повтор через режим застрявшей монеты, не больше
//! stuck.unsent_retries_max раз (sell_unsent.rs); исход продажи неизвестен — сделка остановлена до ручной проверки.
//! Решение 148: сделка закрыта продажей — сверка средней цены продажи с планом (sell_slip.rs).
//! Кто вызывает: executor.rs, stuck.rs, manual.rs.
//! От чего зависит: trade_ops.rs, fills.rs, ledger.rs, stuck.rs, manual.rs, deal.rs, sell_slip.rs, sell_unsent.rs.
//! Настройки: через sell_unsent.rs — stuck.unsent_retries_max (решение 110), stuck.reject_retry_pause_ms (решение 154).

use rust_decimal::Decimal;

use crate::gateway::errors::ErrorCode;

use crate::core::types::{Quote, Side};

use super::deal::{Deal, Step};
use super::engine::Core;

/// Причина продажи кнопкой «Продать сейчас» (manual.rs, manual_halted.rs). Решение 110: продажу, которую нажал
/// человек, бот не уводит в режим застрявшей монеты — сбой сразу алертом.
pub const BUTTON: &str = "кнопка «Продать сейчас»";

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
            // Возврат USDT на площадку покупки — общий шаг закрытия (ledger::finish, решение 138).
            super::ledger::finish(core, &mut deal, Step::Closed, &format!("продано ({why})")).await;
            // Решение 148: факт продажи против плана — серия хуже плана уводит монету в чёрный список.
            super::sell_slip::after_close(core, &deal).await;
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
        // Исход продажи неизвестен (транзакция могла исполниться) — ни продавать, ни перегонять: ручная проверка.
        Err(e) if e.outcome_unknown() => {
            super::ledger::finish(
                core,
                &mut deal,
                Step::Halted,
                &format!("исход продажи неизвестен: {e} — проверьте вручную"),
            )
            .await;
        }
        // Решение 100: продажа не прошла (откат, не попала в блок, цена хуже допустимой, отказ площадки) — монеты на
        // месте; бот сам не повторяет (стартовое правило), а ставит алерт человеку: «Продать сейчас» (по реальному
        // балансу), «Беру на себя», «Продано вручную» — в интерфейсе и кнопками Telegram.
        // Решения 110 и 154: продажа не ушла в сеть (отказ до отправки; или ни один канал не принял, и узел подтвердил,
        // что транзакции нет ни в блоке, ни в пуле) — денег и газа не стоила, стартовое правило («ордер отправлен и не
        // исполнился») не про неё: повтор через режим застрявшей монеты, не больше stuck.unsent_retries_max раз
        // (sell_unsent.rs). Строку хронологии об отказе пишет алерт (manual::raise_alert) — без дубля (решение 154).
        Err(e) => match super::sell_unsent::decide(core, &deal, &e, why) {
            super::sell_unsent::Retry::Go { pause_ms } => {
                super::sell_unsent::retry(core, deal, &e, pause_ms).await
            }
            r => super::sell_unsent::alert(core, deal, &e, &r).await,
        },
    }
}
