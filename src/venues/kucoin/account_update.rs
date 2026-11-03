//! Файл: venues/kucoin/account_update.rs
//! Что делает: разбор сообщения приватной темы /account/balance KuCoin в изменение баланса ядра (решение 76).
//! Изменение = availableChange + holdChange (общий баланс счёта: постановка ордера переносит доступное в удержанное —
//! сумма 0, такие сообщения ядру не отдаются). Учитываются счета main и trade (trade_hf — тоже торговый); маржа и
//! прочие счета — пропускаются (бот только спот).
//! Кто вызывает: user_stream_conn.rs, тесты (после первой живой записи — tests/README.md).
//! От чего зависит: num.rs, core/types/balance.rs.
//! Настройки: нет.
//!
//! Официально — Balance https://kucoin.com/docs-new/3470075w0 (спецификация SDK spec/ws/openapi-spot-private.json):
//! data {accountId, currency, total, available, availableChange, hold, holdChange, relationEvent, relationEventId,
//! relationContext {symbol, orderId}, time}; relationEvent: main.deposit «Deposit», main.withdraw_hold «Hold withdrawal
//! amount», main.withdraw_done «Withdrawal done», main.transfer, main.other, trade.hold, trade.setted «Settlement (Trade
//! account)», trade.transfer, trade.other, trade_hf.* — то же для trade_hf, margin.*, isolated_*, other.
//! Хеша транзакции в сообщении нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, BalanceChange, BalanceKind, VenueId};

use super::num;

/// Вид изменения по relationEvent. Возвращает: (вид, своё ли); None — не счёт main/trade. Ошибок нет.
pub fn kind(event: &str) -> Option<(BalanceKind, bool)> {
    let (acc, what) = event.split_once('.')?;
    match (acc, what) {
        ("main", "deposit") => Some((BalanceKind::Deposit, false)),
        ("main", "withdraw_done") | ("main", "withdraw_hold") => {
            Some((BalanceKind::Withdraw, false))
        }
        // Ордера и переводы между своими счетами делает бот (аккаунт только его — решение 76).
        ("trade" | "trade_hf", "hold" | "setted" | "transfer") | ("main", "transfer") => {
            Some((BalanceKind::Other, true))
        }
        ("main" | "trade" | "trade_hf", _) => Some((BalanceKind::Other, false)),
        _ => None,
    }
}

/// Разбирает data сообщения. Принимает: data, площадку. Возвращает: (relationEvent, изменение; None — сумма 0 или не
/// счёт main/trade). Ошибка — текст причины (поле не число, нет монеты).
pub fn parse(data: &Value, venue: &VenueId) -> Result<(String, Option<BalanceChange>), String> {
    let event = num::text_opt(data, "relationEvent").unwrap_or_default();
    let currency = num::text_opt(data, "currency").ok_or("нет currency")?;
    let dec = |f: &str| {
        num::dec_opt(data, f)
            .map(|v| v.unwrap_or(Decimal::ZERO))
            .map_err(|_| format!("{currency}: {f} не число"))
    };
    let delta = dec("availableChange")? + dec("holdChange")?;
    let Some((kind, own)) = kind(&event) else {
        return Ok((event, None));
    };
    if delta.is_zero() {
        return Ok((event, None));
    }
    let at = num::text_opt(data, "time")
        .and_then(|t| t.parse::<i64>().ok())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    let change = BalanceChange {
        venue: venue.clone(),
        asset: Asset {
            ticker: currency,
            bsc_contract: None,
        },
        delta,
        kind,
        own,
        tx_hash: None,
        at,
        source: format!("KuCoin /account/balance relationEvent={event}"),
        account: None,
    };
    Ok((event, Some(change)))
}
