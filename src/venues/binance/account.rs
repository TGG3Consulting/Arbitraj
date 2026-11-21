//! Файл: venues/binance/account.rs
//! Что делает: балансы спот-счёта Binance (свободно и в ордерах по каждой монете).
//! Кто вызывает: mod.rs (balances), discount_balance.rs, all_balances.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официально: GET /api/v3/account (USER_DATA, вес 20, rest-api «Account information») — omitZeroBalances («When set
//! to true, emits only the non-zero balances»), balances[]: asset, free, locked; canTrade, canWithdraw, canDeposit,
//! permissions. Баланса одной монеты у спот API нет — balance(asset) контракта по умолчанию фильтрует весь список.

use reqwest::Method;

use crate::core::types::Balance;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Ответ GET /api/v3/account с ненулевыми балансами. Ошибки: ключи, связь, ответ Binance.
pub async fn info(ctx: &Ctx) -> GatewayResult<serde_json::Value> {
    let p = vec![("omitZeroBalances", "true".to_string())];
    rest::signed(ctx, Method::GET, "/api/v3/account", p, Op::Other).await
}

/// Балансы спот-счёта (только ненулевые). Ошибки: ключи, связь, ответ Binance (нет balances, не число).
pub async fn balances(ctx: &Ctx) -> GatewayResult<Vec<Balance>> {
    let data = info(ctx).await?;
    parse(ctx, &data)
}

/// Разбор balances[] ответа account. Ошибка BadResponse — нет массива или поле не число.
pub fn parse(ctx: &Ctx, data: &serde_json::Value) -> GatewayResult<Vec<Balance>> {
    num::arr(data, "balances")?
        .iter()
        .map(|b| {
            Ok(Balance {
                venue: ctx.venue(),
                asset: num::text(b, "asset")?,
                free: num::dec(b, "free")?,
                locked: num::dec(b, "locked")?,
            })
        })
        .collect()
}
