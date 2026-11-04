//! Файл: venues/okx/deposit_event.rs
//! Что делает: разбор строки канала deposit-info OKX в зачисление депозита для ядра (решение 76): событие баланса
//! «депозит» с хешем транзакции — при переходе депозита в состояние 1 «deposit credited» или 2 «deposit successful»,
//! один раз на депозит (depId).
//! Кто вызывает: user_stream_conn.rs, тесты (после первой живой записи — tests/README.md).
//! От чего зависит: num.rs, core/types/balance.rs.
//! Настройки: нет.
//!
//! Официально — «Funding Account → WebSocket → Deposit info channel» https://www.okx.com/docs-v5/en/ (адрес
//! /ws/v5/business, нужен вход): «A push notification is triggered when a deposit is initiated or the deposit status
//! changes»; поля data — uid, subAcct («returns "" if the producer is the master account»), pTime («Push time … Unix
//! timestamp format in milliseconds»), ccy, chain, amt, from, to («Deposit address»), txId («Hash record of the
//! deposit»), ts, state (0 «waiting for confirmation», 1 «deposit credited», 2 «deposit successful», 8, 11, 12, 13, 14),
//! depId, fromWdId, actualDepBlkConfirm. Счёт зачисления (Funding / Trading) в событии не указан — его знает адрес
//! депозита (deposit_addr.rs); перевод на Trading — deposit_to_trading.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{Asset, BalanceChange, BalanceKind, VenueId};

use super::num;

/// Что делает: строка data → зачисление, если депозит впервые стал «зачислен» (state 1 или 2) и не субсчёта.
/// Принимает: строку, площадку, уже отданные depId (дополняется). Возвращает: (state, depId, зачисление или None).
/// Ошибка — текст причины (нет поля, сумма не число).
pub fn parse(
    row: &Value,
    venue: &VenueId,
    seen: &mut HashSet<String>,
) -> Result<(String, String, Option<BalanceChange>), String> {
    let state = num::text_opt(row, "state").ok_or("нет state")?;
    let dep_id = num::text_opt(row, "depId").ok_or("нет depId")?;
    let own_account = num::text_opt(row, "subAcct").is_none();
    if !matches!(state.as_str(), "1" | "2") || !own_account || seen.contains(&dep_id) {
        return Ok((state, dep_id, None));
    }
    let ccy = num::text_opt(row, "ccy").ok_or("нет ccy")?;
    let amt = num::dec(row, "amt").map_err(|e| e.message)?;
    seen.insert(dep_id.clone());
    let at = num::text_opt(row, "pTime")
        .and_then(|t| t.parse::<i64>().ok())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    let change = BalanceChange {
        venue: venue.clone(),
        asset: Asset {
            ticker: ccy,
            bsc_contract: None,
        },
        delta: amt,
        kind: BalanceKind::Deposit,
        own: false,
        tx_hash: num::text_opt(row, "txId"),
        at,
        source: format!("OKX deposit-info state={state} depId={dep_id}"),
        account: None,
    };
    Ok((state, dep_id, Some(change)))
}
