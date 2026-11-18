//! Файл: core/sell_fact.rs
//! Что делает: решение 413 (t198, живой WKC 18.11: продажа записи депозита 6 008 053 740.36 → MEXC 30005 Oversold →
//! «ждём решения человека») — факт баланса перед продажей на БИРЖЕ и отказ «мало средств» без человека.
//!
//! before (вход в продажу sell.rs: sell_or_stuck, market_guarded — первая продажа после перевода, «Вернуть боту»,
//! остаток, после перегона, режим застрявшей монеты, кнопки; ревью t198, Б2 — и перед ВЫВОДОМ монеты сделки с биржи и
//! оценкой перегона: relay.rs «Перегон», manual_relay.rs «Продать и перегнать»): ВСЕГДА на бирже (ревью t198, С1 —
//! и у монет без налога; «никаких если») свежий факт счёта (remain_fact::reconcile под замком счёта) и доля сделки —
//! min(доля по факту, факт − доли и учёт других живых сделок). Сделка ждёт доли, а через счёт идёт сделка в пути —
//! сначала ждём её зачисления (wait_transit). Повторного чтения из потока баланса нет: событие приватного потока
//! с балансом счёта (не только изменением) по коду ядра не хранится — не гадаем.
//! DEX и нативная BNB — без изменений (DEX считает факт продажи по балансу кошелька, решение 222).
//!
//! retry (отказ продажи на бирже с кодом InsufficientBalance, любой биржи: 10101/30004/30005 MEXC и т. п.): ордер не
//! поставлен, денег не тронуто — отметка reread = количество отказа (подтверждённое только записью — снова «ждёт доли»),
//! снова вход в продажу тем же правилом (before перечитает факт и пересчитает долю). Повторный отказ после свежего
//! факта (или факт не меньше количества отказа: дело не в доле) — прежний путь: алерт человеку с цифрами в хронологии.
//! Кто вызывает: core/sell.rs, core/relay.rs, core/manual_relay.rs.
//! От чего зависит: remain_fact.rs, credit_fact.rs, deposit_hold.rs, sell_unsent.rs (alert), manual_outcome.rs,
//! trade_ops.rs, native.rs, sell_range.rs (Rule), remeasure.rs (Guard), deal_words.rs, deal.rs.
//! Настройки: transfer.max_time_s, venue.status_poll_ms (ожидание сделки в пути — wait_transit).

use rust_decimal::Decimal;

use crate::gateway::contract::VenueKind;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;
use super::remeasure::Guard;
use super::sell_range::Rule;

/// Что делает: нужен ли свежий факт перед продажей / выводом (ревью t198, С1: на бирже — всегда). Принимает: ядро,
/// сделку. Ошибок нет.
pub fn need(core: &Core, deal: &Deal) -> bool {
    deal.qty > Decimal::ZERO
        && super::trade_ops::kind(core, &deal.at) == Ok(VenueKind::Cex)
        && !super::native::is_native(core, &deal.direction.asset)
}

/// Что делает: факт перед продажей или выводом монеты сделки с биржи (шапка). Принимает: ядро, сделку, причину
/// (продажи или вывода — для итога кнопки). Возвращает: сделку с долей — продавать / выводить; None — дальше не идём
/// (закрыта, алерт человеку; у кнопки — итог кнопки). Ошибок нет.
pub async fn before(core: &Core, mut deal: Deal, why: &str) -> Option<Deal> {
    if !need(core, &deal) {
        return Some(deal);
    }
    // Ревью t198: через счёт идёт сделка в пути — её зачисление делит факт вместе с нашим; ждём его (шапка).
    Box::pin(wait_transit(core, &deal)).await;
    let tried = super::credit_fact::here(&deal).and_then(|c| c.reread);
    if !Box::pin(super::remain_fact::reconcile(core, &mut deal)).await {
        // Ревью t198: путь кнопки — итог кнопки (алерт или закрытие — уже в хронологии и «Ручном управлении»).
        if why == super::sell::BUTTON {
            let text = "по факту счёта у сделки монет нет (алерт или закрытие — см. хронологию сделки, решение 413)";
            Box::pin(super::manual_outcome::failed(core, &deal, why, text)).await;
        }
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

/// Что делает: ревью t198 — сделка ждёт доли, а через счёт идёт сделка той же монеты в пути (её монеты могли уже прийти,
/// а записи депозита ещё нет): распределение подождёт её зачисления — не дольше transfer.max_time_s от нашего
/// зачисления (тот же предел, что у перевода), опрос venue.status_poll_ms или сохранение учёта (coin_wake). Не задана
/// любая — не ждём (доля тогда не выше ожидаемой: credit_split.rs cap). Принимает: ядро, сделку. Ошибок нет.
async fn wait_transit(core: &Core, deal: &Deal) {
    let Some(since) = super::credit_fact::here(deal)
        .filter(|_| super::credit_fact::pending(deal))
        .map(|c| c.at)
    else {
        return;
    };
    let a = &deal.direction.asset;
    let coin = crate::settings::scope::coin_id(&a.ticker, a.bsc_contract.as_ref());
    let s = &core.inner.deps.settings;
    let with_coin = At {
        venue: Some(&deal.at.0),
        coin: Some(&coin),
    };
    let venue_only = At {
        venue: Some(&deal.at.0),
        coin: None,
    };
    let (Ok(max_s), Ok(poll)) = (
        s.integer("transfer.max_time_s", with_coin),
        s.integer("venue.status_poll_ms", venue_only),
    ) else {
        return;
    };
    let max_s = i64::try_from(max_s).unwrap_or(i64::MAX);
    loop {
        let wake = core.inner.coin_wake.notified();
        if super::remain_others::on_account(core, deal, &deal.at)
            .unclear
            .is_empty()
            || (chrono::Utc::now() - since).num_seconds() >= max_s
        {
            return;
        }
        tokio::select! {
            _ = wake => {}
            _ = tokio::time::sleep(std::time::Duration::from_millis(poll)) => {}
        }
    }
}
