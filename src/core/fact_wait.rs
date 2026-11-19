//! Файл: core/fact_wait.rs
//! Что делает: решение 413 (третье ревью t198, С-Д; четвёртое, С-4) — баланс биржи не прочитан перед ПЕРВЫМ выводом
//! монеты сделки после покупки и (пятое ревью, С-3) внутри режима застрявшей монеты: сделка не уходит в режим застрявшей
//! монеты сразу и не уходит к человеку сразу, а ждёт и перечитывает баланс с
//! интервалом venue.status_poll_ms, не дольше transfer.max_time_s (по площадке и монете). При первом сбое — строка в
//! хронологию сделки, запись учёта и одно сообщение deal.no_fact по сделке (строка Warn core.credit_fact — только у
//! первого чтения, повторные — без неё). Сделку тем временем взял человек («Беру на себя», кнопки — запись сделки в
//! учёте изменилась) — ожидание кончается, сделка дальше не ведётся этим путём. Не задан интервал или предел — как
//! раньше: сразу режим застрявшей монеты (credit_alert::unread). Предел вышел — тот же путь с причиной «не дождались».
//! Кто вызывает: core/remain_fact.rs (reconcile_as, Unread::Wait).
//! От чего зависит: remain_read.rs (read_with), credit_alert.rs (wait_ms), ledger.rs, settings, deps.rs, engine.rs.
//! Настройки: venue.status_poll_ms, transfer.max_time_s (существующие).

use rust_decimal::Decimal;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::{Core, lk};
use super::remain_others::Others;

/// Итог ожидания.
pub enum Waited {
    /// Последнее чтение (успех или ошибка — с причиной «не дождались»).
    Read(Result<Decimal, String>, Others),
    /// Сделку взял человек — дальше не ведём.
    Gone,
}

/// transfer.max_time_s сделки (по площадке и монете). None — не задана. Ошибок нет.
fn limit_s(core: &Core, deal: &Deal) -> Option<u64> {
    let a = &deal.direction.asset;
    let coin = crate::settings::scope::coin_id(&a.ticker, a.contract.as_ref());
    let at = At {
        venue: Some(&deal.at.0),
        coin: Some(&coin),
    };
    core.inner
        .deps
        .settings
        .integer("transfer.max_time_s", at)
        .ok()
}

/// Что делает: ожидание и перечитывание (шапка). Принимает: ядро, сделку, ошибку первого чтения, учёт других.
/// Возвращает: Waited. Ошибок нет.
pub async fn first(core: &Core, deal: &mut Deal, err: String, others: Others) -> Waited {
    let (Some(ms), Some(max)) = (
        super::credit_alert::wait_ms(core, &deal.at),
        limit_s(core, deal),
    ) else {
        return Waited::Read(Err(err), others);
    };
    let text = format!(
        "{}: баланс {} на {} не прочитан перед выводом / продажей ({err}) — сделка ждёт и перечитывает каждые {ms} мс, \
         не дольше {max} с (transfer.max_time_s), потом — режим застрявшей монеты, а из него — человеку (решение 413)",
        deal.id, deal.direction.asset.ticker, deal.at.0
    );
    deal.note(text.clone());
    Box::pin(super::ledger::save(core, deal)).await;
    Box::pin(
        core.inner
            .deps
            .notify("deal.no_fact", Some(&deal.at.0), &text),
    )
    .await;
    let mark = lk(&core.inner.deals)
        .get(&deal.id)
        .map(|d| (d.step, d.updated_at));
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(max);
    let (mut read, mut others) = (Err(err), others);
    while read.is_err() && tokio::time::Instant::now() < until {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        let now = lk(&core.inner.deals)
            .get(&deal.id)
            .map(|d| (d.step, d.updated_at));
        if now != mark {
            let t = format!(
                "{}: сделку взял человек — ожидание баланса прекращено",
                deal.id
            );
            Box::pin(core.inner.deps.log(
                Level::Info,
                "core.credit_fact",
                Some(&deal.at.0),
                &t,
                None,
            ))
            .await;
            return Waited::Gone;
        }
        (read, others) = Box::pin(super::remain_read::read_with(core, deal, false)).await;
    }
    let read = read.map_err(|e| format!("{e}; не дождались за {max} с (transfer.max_time_s)"));
    Waited::Read(read, others)
}
