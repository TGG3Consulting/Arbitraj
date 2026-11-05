//! Файл: venues/binance/account_update.rs
//! Что делает: разбор события потока аккаунта Binance (WebSocket API, обёртка {"subscriptionId", "event"}) в
//! изменение баланса ядра. balanceUpdate — изменение баланса монеты (d — дельта) при депозите, выводе или переводе
//! между счетами; outboundAccountPosition (полные остатки изменённых монет) и executionReport (ордер) — не изменения
//! «извне»: ядру не отдаются (итог ордера — из ответа REST, trade.rs).
//! Кто вызывает: user_stream_conn.rs, тесты (после первой живой записи — tests/README.md).
//! От чего зависит: core/types/balance.rs, serde_json.
//! Настройки: нет.
//!
//! Официально (https://developers.binance.com/legacy-docs/binance-spot-api-docs/user-data-stream): Balance Update —
//! {"e":"balanceUpdate","E":время,"a":монета,"d":"дельта","T":время расчёта}; «occurs during deposits or withdrawals
//! from the account; transfer of funds between accounts». Хеша транзакции и вида изменения в событии нет: вид — по
//! знаку дельты (плюс — Deposit, минус — Withdraw), «своё ли» — решает ядро по своим переводам (own = false).
//! eventStreamTerminated — подписка закрыта сервером: переподключиться.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, BalanceChange, BalanceKind, VenueId};

/// Разобранное событие.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Не изменение баланса (другое событие или ответ на запрос).
    Other(String),
    /// Изменение баланса монеты.
    Balance(BalanceChange),
    /// Сервер закрыл подписку (eventStreamTerminated).
    Terminated,
    /// Событие не разобрано — причина.
    Bad(String),
}

/// Разбирает текстовый кадр WebSocket API. Принимает: текст, площадку. Возвращает: Event. Ошибок нет.
pub fn parse(text: &str, venue: &VenueId) -> Event {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Event::Bad("кадр не JSON".into());
    };
    let Some(ev) = v.get("event") else {
        return Event::Other(String::new());
    };
    let kind = ev.get("e").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "balanceUpdate" => balance(ev, venue),
        "eventStreamTerminated" => Event::Terminated,
        other => Event::Other(other.to_string()),
    }
}

/// balanceUpdate → изменение. Ошибок нет: неразборчивое — Event::Bad.
fn balance(ev: &Value, venue: &VenueId) -> Event {
    let Some(asset) = ev
        .get("a")
        .and_then(Value::as_str)
        .filter(|a| !a.is_empty())
    else {
        return Event::Bad("balanceUpdate без монеты a".into());
    };
    let Some(delta) = ev
        .get("d")
        .and_then(Value::as_str)
        .and_then(|d| d.parse::<Decimal>().ok())
    else {
        return Event::Bad(format!("balanceUpdate {asset}: d не число"));
    };
    if delta.is_zero() {
        return Event::Other("balanceUpdate 0".into());
    }
    let at = ev
        .get("T")
        .or_else(|| ev.get("E"))
        .and_then(Value::as_i64)
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    let kind = if delta > Decimal::ZERO {
        BalanceKind::Deposit
    } else {
        BalanceKind::Withdraw
    };
    Event::Balance(BalanceChange {
        venue: venue.clone(),
        asset: Asset {
            ticker: asset.to_string(),
            bsc_contract: None,
        },
        delta,
        kind,
        own: false,
        tx_hash: None,
        at,
        source: "Binance userDataStream balanceUpdate".into(),
        account: None,
    })
}
