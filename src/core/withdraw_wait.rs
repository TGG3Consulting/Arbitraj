//! Файл: core/withdraw_wait.rs
//! Что делает: решение 253 — перед выводом монеты с площадки дождаться, пока она доступна для вывода (живой случай
//! Тиграна 10.11: 12 сделок Binance → DEX, вывод через 0.35–0.77 с после покупки отказан «−4026 insufficient balance» /
//! «−4024 does not own this currency»). Задана venue.withdraw_ready_wait_ms площадки-отправителя — контракт
//! withdraw_ready адаптера (у Binance — событие баланса потока аккаунта, по сроку — один запрос REST; у прочих
//! площадок по умолчанию — «не сообщает», без ожидания); не задана — вывод сразу, как раньше. Сколько ждали и чем
//! кончилось — строка хронологии сделки «Ожидание доступности монеты для вывода: N мс …» и журнал core.withdraw_wait.
//! Ожидание входит во время в пути (transfers.rs: начало перевода — до ожидания; transit.rs «авто» — по факту).
//! Ответ «свободно меньше суммы» или сбой проверки вывод не отменяют: вывод уходит, его отказ закрывает повтор
//! решения 166 (transfer_locked.rs) — у Binance по окну venue.withdraw_unready_window_ms (venues/binance/transfers.rs).
//! Дополнение (Тигран 10.11): страховка venue.withdraw_min_delay_ms — вывод не раньше этой паузы после исполнения
//! покупки сделки на этой площадке (TradeResult.finished_at покупки); действует вместе с ожиданием по событию (сначала
//! пауза, потом ожидание) и повтором −4026/−4024; строка «Пауза перед выводом после покупки: N мс …», во время в пути
//! входит так же. Покупки сделки на этой площадке нет (перегон, возврат USDT) или пауза уже прошла — без паузы.
//! Кто вызывает: core/transfers.rs (go — перед выводом).
//! От чего зависит: engine.rs, ledger.rs (хронология), gateway/contract.rs (withdraw_ready), settings.
//! Настройки: venue.withdraw_ready_wait_ms, venue.withdraw_min_delay_ms (по площадке-отправителю, необязательные, без
//! значений по умолчанию).

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
    let s = &core.inner.deps.settings;
    let paused = pause(
        core,
        deal,
        from,
        s.integer("venue.withdraw_min_delay_ms", at).ok(),
    )
    .await;
    let Ok(ms) = s.integer("venue.withdraw_ready_wait_ms", at) else {
        return paused;
    };
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
    Some(waited + paused.unwrap_or_default())
}

/// Что делает: страховка venue.withdraw_min_delay_ms (см. шапку) — пауза до «исполнение покупки сделки на этой площадке
/// плюс delay_ms». Принимает: ядро, id сделки, площадку, паузу (None — не задана). Возвращает: сколько ждали (None — не
/// ждали). Ошибок нет.
async fn pause(core: &Core, deal: &str, from: &VenueId, delay_ms: Option<u64>) -> Option<Duration> {
    let ms = delay_ms?;
    let bought = {
        let deals = lk(&core.inner.deals);
        deals
            .get(deal)
            .and_then(|d| d.buy.as_ref())
            .filter(|b| &b.request.venue == from)
            .map(|b| b.finished_at)
    }?;
    let left = left_ms(bought, chrono::Utc::now(), ms)?;
    tokio::time::sleep(left).await;
    let text = format!(
        "Пауза перед выводом после покупки: {} мс (venue.withdraw_min_delay_ms = {ms})",
        left.as_millis()
    );
    note(core, deal, from, &text).await;
    Some(left)
}

/// Что делает: сколько ещё ждать до «покупка + ms» (чистая функция). Принимает: время исполнения покупки, сейчас, паузу.
/// Возвращает: остаток; None — пауза уже прошла. Ошибок нет.
pub fn left_ms(
    bought: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    ms: u64,
) -> Option<Duration> {
    let until = bought + chrono::Duration::milliseconds(i64::try_from(ms).ok()?);
    (until > now).then(|| (until - now).to_std().ok()).flatten()
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
