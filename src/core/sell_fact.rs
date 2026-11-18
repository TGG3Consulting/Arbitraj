//! Файл: core/sell_fact.rs
//! Что делает: решение 413 (t198, живой WKC 18.11: продажа записи депозита 6 008 053 740.36 → MEXC 30005 Oversold →
//! «ждём решения человека») — факт баланса перед продажей на БИРЖЕ и отказ «мало средств» без человека.
//!
//! before (вход в продажу sell.rs: sell_or_stuck, market_guarded — первая продажа после перевода, «Вернуть боту»,
//! остаток, после перегона, режим застрявшей монеты, кнопки): свежий факт счёта (remain_fact::reconcile под замком
//! счёта) и продажа доли сделки — min(доля по факту, факт − доли и учёт других живых сделок), если:
//! - зачисление сделки ждёт доли (credit_fact.rs), или
//! - была отметка отказа «мало средств» (reread), или
//! - монета «подозрительная» на этой бирже (deposit_hold.rs suspect: ожидаемое удержание > 0 или налог токена не
//!   измерен).
//!
//! Иначе (монета без налога токена и без замеренного удержания, зачисление подтверждено записью) — без чтения: запись =
//! зачисленное, продажа не ждёт лишний запрос к бирже; если всё же «мало средств» — путь ниже.
//! DEX и нативная BNB — без изменений (DEX считает факт продажи по балансу кошелька, решение 222).
//!
//! retry (отказ продажи на бирже с кодом InsufficientBalance, любой биржи: 10101/30004/30005 MEXC и т. п.): ордер не
//! поставлен, денег не тронуто — отметка reread = количество отказа (подтверждённое только записью — снова «ждёт доли»),
//! снова вход в продажу тем же правилом (before перечитает факт и пересчитает долю). Повторный отказ после свежего
//! факта (или факт не меньше количества отказа: дело не в доле) — прежний путь: алерт человеку с цифрами в хронологии.
//! Кто вызывает: core/sell.rs.
//! От чего зависит: remain_fact.rs, credit_fact.rs, deposit_hold.rs, sell_unsent.rs (alert), manual_outcome.rs,
//! trade_ops.rs, native.rs, sell_range.rs (Rule), remeasure.rs (Guard), deal_words.rs, deal.rs.
//! Настройки: нет (базовое поведение).

use rust_decimal::Decimal;

use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;
use super::remeasure::Guard;
use super::sell_range::Rule;

/// Что делает: нужен ли свежий факт перед продажей (шапка). Принимает: ядро, сделку. Ошибок нет.
pub fn need(core: &Core, deal: &Deal) -> bool {
    let a = &deal.direction.asset;
    deal.qty > Decimal::ZERO
        && super::trade_ops::kind(core, &deal.at) == Ok(VenueKind::Cex)
        && !super::native::is_native(core, a)
        && (super::credit_fact::pending(deal)
            || super::credit_fact::here(deal).is_some_and(|c| c.reread.is_some())
            || super::deposit_hold::suspect(core, &deal.at, a))
}

/// Что делает: факт перед продажей (шапка). Принимает: ядро, сделку, причину продажи. Возвращает: сделку с долей —
/// продавать; None — дальше не продаём (закрыта, алерт человеку). Ошибок нет.
pub async fn before(core: &Core, mut deal: Deal, why: &str) -> Option<Deal> {
    if !need(core, &deal) {
        return Some(deal);
    }
    let tried = super::credit_fact::here(&deal).and_then(|c| c.reread);
    if !Box::pin(super::remain_fact::reconcile(core, &mut deal)).await {
        return None;
    }
    match tried {
        // Свежий факт не меньше количества отказа — дело не в доле: прежний путь, человеку.
        Some(t) if deal.qty >= t => {
            let e = OpError::from(format!(
                "биржа ответила «мало средств» на {}, а по свежему факту у сделки {} — дело не в доле сделки \
                 (решение 413)",
                super::deal_words::coins(t, &deal.direction.asset.ticker),
                super::deal_words::coins(deal.qty, &deal.direction.asset.ticker)
            ));
            // Цепочка повтора кончилась: отметка снимается — следующая кнопка человека не упрётся в неё.
            super::credit_fact::clear_reread(&mut deal);
            let d = deal.clone();
            let r = super::sell_unsent::Retry::No;
            Box::pin(super::sell_unsent::alert(core, deal, &e, &r)).await;
            Box::pin(super::manual_outcome::failed(core, &d, why, &e.message)).await;
            None
        }
        _ => Some(deal),
    }
}

/// Что делает: отказ продажи «мало средств» на бирже — повторить с долей по свежему факту (шапка)? Повтор — только
/// если на этом количестве ещё не было отказа (иначе — прежний путь, алерт). Принимает: ядро, сделку, ошибку продажи.
/// Возвращает: да — повторить (retry). Ошибок нет.
pub fn wanted(core: &Core, deal: &Deal, e: &OpError) -> bool {
    e.code == Some(ErrorCode::InsufficientBalance)
        && !e.sent
        && super::trade_ops::kind(core, &deal.at) == Ok(VenueKind::Cex)
        && !super::native::is_native(core, &deal.direction.asset)
        && super::credit_fact::here(deal)
            .and_then(|c| c.reread)
            .is_none()
}

/// Что делает: повтор после «мало средств»: отметка, хронология, журнал core.credit_fact, снова вход в продажу тем же
/// правилом (Entry — sell_or_stuck; кнопка — market BUTTON; порог режима — market_guarded с ним; без порога — Off).
/// Принимает: ядро, сделку, ошибку, причину продажи, правило. Ошибок нет.
pub async fn retry(core: &Core, mut deal: Deal, e: &OpError, why: &str, rule: Rule) {
    let tried = deal.qty;
    super::credit_fact::mark_reread(&mut deal, tried);
    let text = format!(
        "{}: биржа отказала в продаже {} — «мало средств» ({e}); ордер не поставлен, деньги не тронуты. Бот \
         перечитывает баланс счёта, пересчитывает долю сделки и продаёт её (решение 413)",
        super::deal_words::deal(core, &deal),
        super::deal_words::coins(tried, &deal.direction.asset.ticker)
    );
    deal.note(text.clone());
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.credit_fact",
            Some(&deal.at.0),
            text,
            None,
        )
        .await;
    match rule {
        Rule::Entry => Box::pin(super::sell::sell_or_stuck(core, deal)).await,
        Rule::Button => Box::pin(super::sell::market(core, deal, super::sell::BUTTON)).await,
        Rule::MaxLoss(m) => {
            Box::pin(super::sell::market_guarded(
                core,
                deal,
                why,
                Guard::MaxLoss(m),
            ))
            .await
        }
        Rule::NoThreshold => {
            Box::pin(super::sell::market_guarded(core, deal, why, Guard::Off)).await
        }
    }
}
