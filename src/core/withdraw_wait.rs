//! Файл: core/withdraw_wait.rs
//! Что делает: решение 251 — перед выводом монеты с площадки дождаться, пока она доступна для вывода (живой случай
//! Тиграна 10.11: 12 сделок Binance → DEX, вывод через 0.35–0.77 с после покупки отказан «−4026 insufficient balance» /
//! «−4024 does not own this currency»). Задана venue.withdraw_ready_wait_ms площадки-отправителя — контракт
//! withdraw_ready адаптера (у Binance — событие баланса потока аккаунта, по сроку — один запрос REST; у прочих
//! площадок по умолчанию — «не сообщает», без ожидания); не задана — вывод сразу, как раньше. Сколько ждали и чем
//! кончилось — строка хронологии сделки «Ожидание доступности монеты для вывода: N мс …» и журнал core.withdraw_wait.
//! Ожидание входит во время в пути (transfers.rs: начало перевода — до ожидания; transit.rs «авто» — по факту).
//! Ответ «свободно меньше суммы» или сбой проверки вывод не отменяют: вывод уходит, его отказ закрывает повтор
//! решения 166 (transfer_locked.rs) — у Binance по окну venue.withdraw_unready_window_ms (venues/binance/transfers.rs).
//! Кто вызывает: core/transfers.rs (go — перед выводом).
//! От чего зависит: engine.rs, ledger.rs (хронология), gateway/contract.rs (withdraw_ready), settings.
//! Настройки: venue.withdraw_ready_wait_ms (по площадке-отправителю, необязательная, без значения по умолчанию).

use std::sync::Arc;
use std::time::{Duration, Instant};

use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId, WithdrawReady};
use crate::gateway::contract::Venue;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk};

/// Что делает: ожидание доступности монеты перед выводом (см. шапку). Принимает: ядро, id сделки, площадку-отправителя,
/// её адаптер, монету (тикер площадки), сумму вывода. Возвращает: сколько ждали (None — настройка не задана, не ждали).
/// Ошибок нет: сбой проверки — строка хронологии, вывод идёт дальше.
pub async fn before(
    core: &Core,
    deal: &str,
    from: &VenueId,
    gw: &Arc<dyn Venue>,
    asset: &Asset,
    qty: Decimal,
) -> Option<Duration> {
    let at = At {
        venue: Some(&from.0),
        coin: None,
    };
    let ms = core
        .inner
        .deps
        .settings
        .integer("venue.withdraw_ready_wait_ms", at)
        .ok()?;
    let t0 = Instant::now();
    let r = gw
        .withdraw_ready(asset, qty, Duration::from_millis(ms))
        .await;
    let waited = t0.elapsed();
    let text = line(
        waited.as_millis(),
        &asset.ticker,
        qty,
        r.map_err(|e| e.message),
    );
    note(core, deal, from, &text).await;
    Some(waited)
}

/// Что делает: строка хронологии по ответу площадки. Принимает: сколько ждали (мс), монету, сумму, ответ или текст сбоя.
/// Возвращает: строку. Ошибок нет.
pub fn line(ms: u128, ticker: &str, qty: Decimal, r: Result<WithdrawReady, String>) -> String {
    let head = format!("Ожидание доступности монеты для вывода: {ms} мс");
    match r {
        Ok(WithdrawReady {
            ready: Some(true),
            free,
            source,
        }) => format!(
            "{head} — свободно {} ≥ {qty} {ticker} ({source})",
            free.map(|f| f.normalize().to_string()).unwrap_or_default()
        ),
        Ok(WithdrawReady {
            ready: Some(false),
            free,
            source,
        }) => format!(
            "{head} — свободно {} < {qty} {ticker} ({source}); вывод отправлен, отказ закроет повтор (transfer.locked_retry_ms / transfer.locked_retries_max)",
            free.map(|f| f.normalize().to_string()).unwrap_or_default()
        ),
        Ok(WithdrawReady {
            ready: None,
            source,
            ..
        }) if source.is_empty() => {
            format!("{head} — площадка доступность вывода не сообщает, вывод сразу")
        }
        Ok(WithdrawReady {
            ready: None,
            source,
            ..
        }) => format!("{head} — {source}, вывод сразу"),
        Err(e) => format!("{head} — проверка не удалась ({e}), вывод сразу"),
    }
}

/// Строка в хронологию сделки (и сохранить) и журнал core.withdraw_wait. Сделки нет — только журнал. Ошибок нет.
async fn note(core: &Core, deal: &str, from: &VenueId, text: &str) {
    let d = {
        let mut deals = lk(&core.inner.deals);
        deals.get_mut(deal).map(|d| {
            d.note(text.to_string());
            d.clone()
        })
    };
    if let Some(d) = d {
        super::ledger::save(core, &d).await;
    }
    core.inner
        .deps
        .log(
            Level::Info,
            "core.withdraw_wait",
            Some(&from.0),
            format!("Сделка {deal}: {text}"),
            None,
        )
        .await;
}
