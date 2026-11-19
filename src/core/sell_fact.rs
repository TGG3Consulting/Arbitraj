//! Файл: core/sell_fact.rs
//! Что делает: решение 413 (t198, живой WKC 18.11: продажа записи депозита 6 008 053 740.36 → MEXC 30005 Oversold →
//! «ждём решения человека») — факт баланса перед продажей на БИРЖЕ и отказ «мало средств» без человека.
//!
//! before (вход в продажу sell.rs: sell_or_stuck, market_guarded — первая продажа после перевода, «Вернуть боту»,
//! остаток, после перегона, режим застрявшей монеты, кнопки; ревью t198, Б2 — и перед ВЫВОДОМ монеты сделки с биржи и
//! оценкой перегона: relay.rs «Перегон», manual_relay.rs «Продать и перегнать»): ВСЕГДА на бирже (ревью t198, С1 —
//! и у монет без налога; «никаких если») свежий факт счёта (remain_fact::reconcile под замком счёта) и доля сделки —
//! min(доля по факту, факт − доли и учёт других живых сделок). Сделка в пути через счёт — не ждём (повторное ревью, С-1):
//! доля ограничена ожидаемой (credit_split.rs). Факт свежий (Deal.fact_fresh: тот же шаг, количество, площадка —
//! предыдущий вход подряд) — второй раз не читаем. Повторного чтения из потока баланса нет: событие приватного потока
//! с балансом счёта (не только изменением) по коду ядра не хранится — не гадаем.
//! DEX и нативная BNB — без изменений (DEX считает факт продажи по балансу кошелька, решение 222).
//!
//! retry (отказ продажи на бирже с кодом InsufficientBalance, любой биржи: 10101/30004/30005 MEXC и т. п.): ордер не
//! поставлен, денег не тронуто — отметка reread = количество отказа (подтверждённое только записью — снова «ждёт доли»),
//! снова вход в продажу тем же правилом (before перечитает факт и пересчитает долю). Повторный отказ после свежего
//! факта (или факт не меньше количества отказа: дело не в доле) — прежний путь: алерт человеку с цифрами в хронологии.
//! «417 (ожидание)»: повтор с порогом режима (Rule::MaxLoss) — порог перепроверяется на котировке под ходом
//! (sell_here_ok.rs try_sell_if); не выполнен — алерт (stuck::over_max), а не ордер по рынку.
//! Кто вызывает: core/sell.rs, core/relay.rs, core/manual_relay.rs, core/executor.rs (первый вывод с биржи).
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

/// Причина первого вывода монеты сделки с биржи после покупки (executor.rs): баланс не прочитан — ждать и
/// перечитывать, без продажи (третье ревью t198, С-Д).
pub const WITHDRAW: &str = "вывод";

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
    // Повторное ревью (мелкое): факт уже прочитан при этом шаге, количестве и площадке (предыдущий вход подряд —
    // stuck / «Продать и перегнать» / продажа здесь) — не читаем второй раз.
    if fresh_take(&mut deal) {
        return Some(deal);
    }
    let tried = super::credit_fact::here(&deal).and_then(|c| c.reread);
    use super::credit_alert::Unread;
    // Четвёртое ревью (М-5): итог кнопки — только у кнопок («Продать сейчас», «Продать и перегнать»); перегон после
    // «Вернуть боту» и прочие ручные входы — автоматика (режим застрявшей монеты).
    let mode = if why == super::sell::BUTTON {
        Unread::Button
    } else if why == WITHDRAW || deal.step == super::deal::Step::Stuck {
        // Пятое ревью (С-3): и внутри режима застрявшей монеты — ждать и перечитывать с пределом (fact_wait.rs), алерт
        // человеку — по пределу (credit_alert::unread, шаг Stuck), без повторного входа в режим.
        Unread::Wait
    } else {
        Unread::Stuck
    };
    if !Box::pin(super::remain_fact::reconcile_as(core, &mut deal, mode)).await {
        // Ревью t198: путь кнопки — итог кнопки (алерт или закрытие — уже в хронологии и «Ручном управлении»); баланс не
        // прочитан — итог кнопки уже отправлен с настоящей причиной (третье ревью, С-Г).
        if why == super::sell::BUTTON && !deal.fact_unread {
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

/// Что делает: третье ревью t198 (Б-1) — одноразовая отметка «факт свежий»: забирает её (take) и отвечает, совпадает ли
/// она с шагом, количеством и площадкой сделки. Допустимый пропуск чтения — только «второй вход подряд» без ожидания
/// (relay → sell_here, manual_relay → market, sell_or_stuck → stuck::handle); режим застрявшей монеты, алерт человеку и
/// учёт памяти (кнопки) отметку сбрасывают. Принимает: сделку. Возвращает: да — факт свежий. Ошибок нет.
pub fn fresh_take(deal: &mut Deal) -> bool {
    let fresh = deal.fact_fresh.take();
    fresh.as_deref() == Some(&(deal.step, deal.qty, deal.at.clone()))
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
    // Факт, по которому продавали, уже не свежий — перечитать.
    deal.fact_fresh = None;
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
        // «417 (ожидание)»: порог режима перепроверяется на котировке под ходом очереди (sell_here_ok.rs); не
        // выполнен или котировки нет — алерт человеку (stuck::over_max), а не ордер по рынку.
        Rule::MaxLoss(m) => {
            let how = super::sell_here_ok::How {
                cond: super::sell_here_ok::Cond::MaxLoss(m),
                guard: Guard::MaxLoss(m),
                rule: Rule::MaxLoss(m),
                note: super::sell_here_ok::Note::None,
            };
            let (d, v) =
                match Box::pin(super::sell_here_ok::try_sell_if(core, deal, why, how)).await {
                    super::sell_here_ok::HereOk::NotNow(d, v) => (*d, v),
                    _ => return,
                };
            let (loss, w) = super::stuck::over_max(&d, v, m);
            Box::pin(super::manual::raise_alert(core, d, loss, w, None)).await
        }
        Rule::NoThreshold => {
            Box::pin(super::sell::market_guarded(core, deal, why, Guard::Off)).await
        }
    }
}
