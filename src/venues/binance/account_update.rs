//! Файл: venues/binance/account_update.rs
//! Что делает: разбор события потока аккаунта Binance (WebSocket API, обёртка {"subscriptionId", "event"}) в
//! изменение баланса ядра. balanceUpdate — изменение баланса монеты (d — дельта) при депозите, выводе или переводе
//! между счетами; outboundAccountPosition (полные остатки изменённых монет) и executionReport (ордер) — не изменения
//! «извне»: ядру не отдаются (итог ордера — из ответа REST, trade.rs); остатки outboundAccountPosition — в withdraw_ready.rs.
//! Кто вызывает: user_stream_conn.rs, тесты (после первой живой записи — tests/README.md).
//! От чего зависит: core/types/balance.rs, serde_json.
//! Настройки: нет.
//!
//! Официально (https://developers.binance.com/legacy-docs/binance-spot-api-docs/user-data-stream): Balance Update —
//! {"e":"balanceUpdate","E":время,"a":монета,"d":"дельта","T":время расчёта}; «occurs during deposits or withdrawals
//! from the account; transfer of funds between accounts». Хеша транзакции и вида изменения в событии нет: вид — по
//! знаку дельты (плюс — Deposit, минус — Withdraw), «своё ли» — решает ядро по своим переводам (own = false).
//! eventStreamTerminated — подписка закрыта сервером: переподключиться.
//! Решение 251: outboundAccountPosition — {"e":"outboundAccountPosition","E":время события,"u":время последнего
//! изменения счёта,"B":[{"a":монета,"f":свободно,"l":в ордерах}]}; «is sent any time an account balance has changed and
//! contains the assets that were possibly changed by the event that generated the balance change» — свободный остаток
//! монеты для ожидания доступности вывода (withdraw_ready.rs), ядру не отдаётся.

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
    /// Решение 251: остатки изменённых монет (outboundAccountPosition): (монета, свободно) и время события, мс.
    Position(Vec<(String, Decimal)>, i64),
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
        "outboundAccountPosition" => position(ev),
        "eventStreamTerminated" => Event::Terminated,
        other => Event::Other(other.to_string()),
    }
}

/// Решение 251: outboundAccountPosition → остатки (монета, свободно f) и время E. Ошибок нет: неразборчивое — Event::Bad.
fn position(ev: &Value) -> Event {
    let Some(list) = ev.get("B").and_then(Value::as_array) else {
        return Event::Bad("outboundAccountPosition без массива B".into());
    };
    let mut out = Vec::with_capacity(list.len());
    for b in list {
        let asset = b.get("a").and_then(Value::as_str).filter(|a| !a.is_empty());
        let free = b
            .get("f")
            .and_then(Value::as_str)
            .and_then(|f| f.parse::<Decimal>().ok());
        match (asset, free) {
            (Some(a), Some(f)) => out.push((a.to_string(), f)),
            _ => {
                return Event::Bad(format!(
                    "outboundAccountPosition: запись B без a или f-числа: {b}"
                ));
            }
        }
    }
    let at = ev.get("E").and_then(Value::as_i64).unwrap_or_default();
    Event::Position(out, at)
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
