//! Файл: core/sell_unsent.rs
//! Что делает: продажа не прошла (sell.rs) — повторить её самому или поставить алерт человеку. Повтор — только если
//! продажа не ушла в сеть (денег и газа не стоила) и это не кнопка человека; общий предел — stuck.unsent_retries_max
//! раз на сделку (Deal.unsent_retries); исчерпан — журнал Error и алерт человеку с кнопками («Продать сейчас»,
//! «Беру на себя», «Продано вручную» — manual.rs, решение 100). Всё остальное (ушедшая в сеть и не исполненная
//! продажа, nonce, нет связи) — как раньше, алерт человеку.
//! Кто вызывает: sell.rs.
//! От чего зависит: op_error.rs, deal.rs, stuck.rs, manual.rs, trade_ops.rs, ledger.rs, deps.rs.
//! Настройки: stuck.unsent_retries_max (решение 110), stuck.reject_retry_pause_ms (решение 154).
//!
//! Когда повтор:
//! - решение 110: отказ ДО отправки (цена сборки хуже допустимой, лимит или очередь запросов, нет маршрута, данные
//!   устарели, налоги не проверены, откат при пробном вызове — OpError::retry_safe) — сразу в режим застрявшей монеты;
//! - решение 154 (живой случай 04.11, ASTER: «Ни один канал не принял транзакцию», 4 минуты до кнопки человека): все
//!   каналы отправки ответили отказом, и адаптер своим узлом проверил, что транзакции нет ни в блоке, ни в пуле
//!   (OpError::rejected_unsent) — пауза stuck.reject_retry_pause_ms, затем режим застрявшей монеты.
//!
//! Режим застрявшей монеты (stuck.rs) продаёт заново тем же порядком, что и первая продажа: новая котировка
//! (trade_ops::sell_value), новый запрос и новая транзакция с nonce, взятым заново (у адаптера).

use std::time::Duration;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::Core;
use super::op_error::OpError;

/// Что делать с неудачной продажей.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retry {
    /// Повторить через режим застрявшей монеты после паузы (мс; 0 — без паузы, решение 110).
    Go { pause_ms: u64 },
    /// Повторы исчерпаны: было `max` повторов (stuck.unsent_retries_max) — алерт.
    Exhausted { max: u32 },
    /// Повтор был бы безопасен, но не настроен — причина (какая настройка не задана); алерт.
    Off(String),
    /// Повтор не положен (ушла в сеть, исход под вопросом, кнопка человека) — алерт как раньше.
    No,
}

/// Решает, повторять ли продажу. Принимает: ядро, сделку, ошибку продажи, причину продажи (кнопка человека —
/// sell::BUTTON — не повторяется, решение 110). Возвращает: Retry. Ошибок нет.
pub fn decide(core: &Core, deal: &Deal, e: &OpError, why: &str) -> Retry {
    if why == super::sell::BUTTON {
        return Retry::No;
    }
    let pause_ms = if e.retry_safe() {
        0
    } else if e.rejected_unsent() {
        match setting(core, "stuck.reject_retry_pause_ms") {
            Some(ms) => ms,
            None => {
                return Retry::Off(
                    "повтор продажи после отказа всех каналов не настроен: не задана stuck.reject_retry_pause_ms (решение 154)"
                        .into(),
                );
            }
        }
    } else {
        return Retry::No;
    };
    let max = unsent_max(core);
    if max == 0 {
        return Retry::Off(
            "повторы продажи, не ушедшей в сеть, выключены: stuck.unsent_retries_max не задан или 0".into(),
        );
    }
    if deal.unsent_retries >= max {
        return Retry::Exhausted { max };
    }
    Retry::Go { pause_ms }
}

/// Повтор: счётчик сделки, запись в хронологию и журнал, пауза, затем режим застрявшей монеты (новая котировка и
/// новая транзакция — stuck.rs → sell.rs). Принимает: ядро, сделку, ошибку, паузу (мс). Ошибок нет.
pub async fn retry(core: &Core, mut deal: Deal, e: &OpError, pause_ms: u64) {
    deal.unsent_retries += 1;
    let max = unsent_max(core);
    let wait = if pause_ms > 0 {
        format!("через {pause_ms} мс ")
    } else {
        String::new()
    };
    let text = format!(
        "продажа не ушла в сеть: {e} — повтор {wait}через режим застрявшей монеты ({}/{max})",
        deal.unsent_retries
    );
    deal.note(text.clone());
    super::ledger::save(core, &deal).await;
    core.inner
        .deps
        .log(
            Level::Warn,
            "core.sell_unsent_retry",
            Some(&deal.at.0),
            format!("{}: {text}", deal.id),
            None,
        )
        .await;
    if pause_ms > 0 {
        tokio::time::sleep(Duration::from_millis(pause_ms)).await;
    }
    Box::pin(super::stuck::handle(core, deal)).await
}

/// Алерт человеку: причина (с пояснением, почему бот сам не повторил), убыток при продаже сейчас, кнопки (manual.rs).
/// Повторы исчерпаны — ещё запись Error в журнал. Принимает: ядро, сделку, ошибку, решение decide. Ошибок нет.
pub async fn alert(core: &Core, deal: Deal, e: &OpError, r: &Retry) {
    let why = match r {
        Retry::Exhausted { max } => {
            let why = format!(
                "продажа не прошла: {e} — не ушла в сеть {} раз подряд, повторы исчерпаны (stuck.unsent_retries_max = {max})",
                deal.unsent_retries + 1
            );
            core.inner
                .deps
                .log(
                    Level::Error,
                    "core.sell_unsent_exhausted",
                    Some(&deal.at.0),
                    format!("{}: {why}", deal.id),
                    None,
                )
                .await;
            why
        }
        Retry::Off(off) => format!("продажа не прошла: {e} — {off}"),
        Retry::No | Retry::Go { .. } => format!("продажа не прошла: {e}"),
    };
    // Хронологию пишет raise_alert («… — ждём решения человека»): своей строки здесь нет — без дубля (решение 154).
    let value = super::trade_ops::sell_value(core, &deal.at, &deal.direction.asset, deal.qty).await;
    let loss = value.ok().map(|(x, _)| deal.loss_pct_if(x));
    super::manual::raise_alert(core, deal, loss, why, Some(e.message.clone())).await;
}

/// Предел повторов продажи, не ушедшей в сеть (stuck.unsent_retries_max). Не задано — 0: без повторов, сразу алерт.
fn unsent_max(core: &Core) -> u32 {
    setting(core, "stuck.unsent_retries_max")
        .map(|n| n.min(u32::MAX as u64) as u32)
        .unwrap_or(0)
}

/// Целая настройка общей области. Возвращает: None — не задана или не читается. Ошибок нет.
fn setting(core: &Core, name: &str) -> Option<u64> {
    core.inner.deps.settings.integer(name, At::default()).ok()
}
