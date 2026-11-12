//! Файл: venues/xt/account.rs
//! Что делает: балансы спот-счёта XT.com (свободно и заморожено по каждой монете) — все сразу и одной монеты
//! (решение 113: на пути сделки нужна одна монета, у XT для этого есть свой запрос).
//! Кто вызывает: mod.rs (balances, balance), all_balances.rs, key_check.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официально:
//! - GET /v4/balances (https://doc.xt.com/docs/spot/Balance/GetBalances, `_balance/balancesGet.md`, «10/s/apikey») —
//!   currencies (через запятую, необязательно); result {totalBtcAmount, assets[{currency, currencyId, frozenAmount,
//!   availableAmount, totalAmount, convertBtcAmount}]}.
//! - GET /v4/balance?currency= (https://doc.xt.com/docs/spot/Balance/GetSingleCurrencyAsset, `_balance/balanceGet.md`) — одна монета, те же поля.

use reqwest::Method;
use serde_json::Value;

use crate::core::types::Balance;
use crate::gateway::errors::GatewayResult;

use super::ctx::{Ctx, currency, ticker};
use super::errors::Op;
use super::{num, rest};

/// Ответ GET /v4/balances. Ошибки: ключи, связь, ответ XT.
pub async fn raw(ctx: &Ctx) -> GatewayResult<Value> {
    rest::signed(ctx, Method::GET, "/v4/balances", vec![], None, Op::Other).await
}

/// Балансы спот-счёта (только ненулевые). Ошибки: ключи, связь, ответ XT (нет assets, не число).
pub async fn balances(ctx: &Ctx) -> GatewayResult<Vec<Balance>> {
    let data = raw(ctx).await?;
    parse(ctx, &data)
}

/// Баланс одной монеты (GET /v4/balance). None — монеты на счёте нет. Ошибки: ключи, связь, ответ XT.
pub async fn balance(ctx: &Ctx, asset: &str) -> GatewayResult<Option<Balance>> {
    let p = vec![("currency", currency(asset))];
    let data = rest::signed(ctx, Method::GET, "/v4/balance", p, None, Op::Other).await?;
    if data.is_null() {
        return Ok(None);
    }
    row(ctx, &data).map(Some)
}

/// Разбор assets[] ответа /v4/balances (нулевые строки пропускаются). Ошибка BadResponse — нет массива, поле не число.
pub fn parse(ctx: &Ctx, data: &Value) -> GatewayResult<Vec<Balance>> {
    let mut out = Vec::new();
    for b in num::arr(data, "assets")? {
        let r = row(ctx, b)?;
        if !r.free.is_zero() || !r.locked.is_zero() {
            out.push(r);
        }
    }
    Ok(out)
}

/// Одна строка: currency → тикер ядра, availableAmount → free, frozenAmount → locked. Ошибка BadResponse.
fn row(ctx: &Ctx, b: &Value) -> GatewayResult<Balance> {
    Ok(Balance {
        venue: ctx.venue(),
        asset: ticker(&num::text(b, "currency")?),
        free: num::dec(b, "availableAmount")?,
        locked: num::dec_opt(b, "frozenAmount")?.unwrap_or_default(),
    })
}
