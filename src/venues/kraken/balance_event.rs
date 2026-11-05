//! Файл: venues/kraken/balance_event.rs
//! Что делает: разбор строки обновления канала balances Kraken в изменение баланса ядра (решение 76): депозит и вывод на
//! спот-кошельке (wallet_type spot), один раз на проводку (ledger_id). Сделки, переводы между кошельками и прочее —
//! не изменения «извне»: ядру не отдаются (итог ордера — из REST, trade.rs).
//! Кто вызывает: user_stream_conn.rs, тесты (после первой живой записи — tests/README.md).
//! От чего зависит: num.rs, core/types/balance.rs.
//! Настройки: нет.
//!
//! Официально — https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/balances.md: update data[] — ledger_id,
//! ref_id, timestamp (RFC3339), type (deposit, withdrawal, trade, …), asset, category, wallet_type (spot, earn),
//! wallet_id, amount («The amount of asset change in this event»), fee, balance. Хеша транзакции в событии нет —
//! событие будит проверку депозита по REST (как Binance balanceUpdate). Знак amount у вывода в документации не сказан:
//! вид — по type, дельта — со знаком «−» у вывода (по модулю amount).

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{Asset, BalanceChange, BalanceKind, VenueId};

use super::num;

/// Кошелёк спота — значение wallet_type.
const SPOT: &str = "spot";

/// Что делает: строка data → изменение, если это депозит или вывод на споте и ledger_id ещё не отдан.
/// Принимает: строку, площадку, отданные ledger_id (дополняется). Возвращает: (type, ledger_id, изменение или None).
/// Ошибка — текст причины (нет поля, сумма не число).
pub fn parse(
    row: &Value,
    venue: &VenueId,
    seen: &mut HashSet<String>,
) -> Result<(String, String, Option<BalanceChange>), String> {
    let kind = num::text_opt(row, "type").ok_or("нет type")?;
    let ledger = num::text_opt(row, "ledger_id").ok_or("нет ledger_id")?;
    let wallet = num::text_opt(row, "wallet_type").unwrap_or_default();
    let bk = match kind.as_str() {
        "deposit" => BalanceKind::Deposit,
        "withdrawal" => BalanceKind::Withdraw,
        _ => return Ok((kind, ledger, None)),
    };
    if wallet != SPOT || seen.contains(&ledger) {
        return Ok((kind, ledger, None));
    }
    let asset = num::text_opt(row, "asset").ok_or("нет asset")?;
    let amount = num::dec(row, "amount").map_err(|e| e.message)?.abs();
    if amount.is_zero() {
        return Ok((kind, ledger, None));
    }
    seen.insert(ledger.clone());
    let at = num::text_opt(row, "timestamp")
        .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
        .map_or_else(Utc::now, |t| t.with_timezone(&Utc));
    let delta = if bk == BalanceKind::Deposit {
        amount
    } else {
        -amount
    };
    let change = BalanceChange {
        venue: venue.clone(),
        asset: Asset {
            ticker: asset,
            bsc_contract: None,
        },
        delta,
        kind: bk,
        own: false,
        tx_hash: None,
        at,
        source: format!("Kraken balances type={kind} ledger_id={ledger}"),
        account: None,
    };
    Ok((kind, ledger, Some(change)))
}
