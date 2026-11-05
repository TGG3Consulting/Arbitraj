//! Файл: venues/htx/account_update.rs
//! Что делает: разбор сообщения темы accounts.update#2 потока аккаунта HTX в изменение баланса ядра (решение 76).
//! HTX присылает не изменение, а новый итог balance монеты счёта, поэтому изменение = новый balance − прежний balance
//! этой монеты (память сессии). Первые кадры после подписки — снимок текущих балансов (changeType = null): они только
//! запоминаются. Монеты, которой не было ни в снимке, ни в изменениях, прежний баланс считается 0 (в снимке её не было —
//! баланса не было; толкование «current balances are sent first» — проверить на живой записи). Учитывается только
//! спот-счёт (accountId = id спот-счёта) и accountType trade (loan / interest — маржа, бот только спот).
//! Кто вызывает: user_stream_conn.rs, тесты (после первой живой записи — tests/README.md).
//! От чего зависит: num.rs, ctx.rs (тикер), core/types/balance.rs.
//! Настройки: нет.
//!
//! Официально — «Websocket Account and Order → Subscribe Account Change» https://huobiapi.github.io/docs/spot/v1/en/:
//! accounts.update#2 — «balance and available» вместе; data {currency, accountId, balance, available, changeType,
//! accountType (trade, loan, interest), changeTime, seqNum}; changeType: order-place, order-match, order-refund,
//! order-cancel, order-fee-refund, margin-*, deposit, withdraw, other. Хеша транзакции в сообщении нет.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, BalanceChange, BalanceKind, VenueId};

use super::ctx::ticker;
use super::num;

/// Вид изменения по changeType. Возвращает: (вид, своё ли). Ошибок нет.
pub fn kind(change: &str) -> (BalanceKind, bool) {
    match change {
        "deposit" => (BalanceKind::Deposit, false),
        "withdraw" => (BalanceKind::Withdraw, false),
        // Ордера делает бот (аккаунт только его — решение 76).
        c if c.starts_with("order-") => (BalanceKind::Other, true),
        _ => (BalanceKind::Other, false),
    }
}

/// Прежние итоги balance по тикеру (на сессию потока).
pub type Last = HashMap<String, Decimal>;

/// Что делает: разбирает data сообщения. Принимает: data, id спот-счёта, прежние итоги, площадку.
/// Возвращает: (changeType или «снимок», изменение; None — снимок, сумма 0, другой счёт или без balance).
/// Ошибка — текст причины (нет currency, поле не число).
pub fn parse(
    data: &Value,
    spot_id: i64,
    last: &mut Last,
    venue: &VenueId,
) -> Result<(String, Option<BalanceChange>), String> {
    let change = num::text_opt(data, "changeType");
    let what = change.clone().unwrap_or_else(|| "снимок".into());
    let account = num::int(data, "accountId").map_err(|e| e.message)?;
    let acc_type = num::text_opt(data, "accountType");
    if account != spot_id || acc_type.as_deref().is_some_and(|t| t != "trade") {
        return Ok((what, None));
    }
    let t = ticker(&num::text_opt(data, "currency").ok_or("нет currency")?);
    let Some(balance) =
        num::dec_opt(data, "balance").map_err(|_| format!("{t}: balance не число"))?
    else {
        return Ok((what, None));
    };
    let before = last.insert(t.clone(), balance);
    let Some(change) = change else {
        return Ok((what, None));
    };
    let delta = balance - before.unwrap_or(Decimal::ZERO);
    if delta.is_zero() {
        return Ok((what, None));
    }
    let (kind, own) = kind(&change);
    let at = num::text_opt(data, "changeTime")
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    let c = BalanceChange {
        venue: venue.clone(),
        asset: Asset {
            ticker: t,
            bsc_contract: None,
        },
        delta,
        kind,
        own,
        tx_hash: None,
        at,
        source: format!("HTX accounts.update changeType={change}"),
        account: None,
    };
    Ok((what, Some(c)))
}
