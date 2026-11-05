//! Файл: venues/phemex/all_balances.rs
//! Что делает: балансы ВСЕХ счетов Phemex для панели «Балансы» (решение 144, урок 24) — только чтение: Спот, Маржа,
//! Фьючерсы USDT-M и UTA (по монетам venue.futures_balance_currencies — у этих запросов монета обязательна), плюс
//! счета без запроса баланса по REST — строкой с причиной. Запросы — параллельно; сбой одного счёта — в его error.
//! Кто вызывает: mod.rs (Venue::account_balances), панель «Балансы» раз в ui.balances_refresh_ms, пока открыта.
//! От чего зависит: rest.rs, account.rs (строки спота), num.rs, symbols.rs, ctx.rs.
//! Настройки: venue.futures_balance_currencies [phemex]; как у rest.rs.
//!
//! Официально (https://phemex-docs.github.io/):
//! - Спот — Query wallets GET /spot/wallets (account.rs).
//! - Маржа — Margin Trading API → Query wallets GET /margin-trade/wallets (#query-wallets-2): [{currency, balanceRq,
//!   lockedTradingBalanceRq, lockedWithdrawRq, borrowedRq, …}] — суффикс Rq в разделе масштабов не описан; в примере —
//!   строки настоящих значений («"0"»), так и читаются (проверить на живом).
//! - Фьючерсы USDT-M — Query Account Positions GET /g-accounts/accountPositions?currency= — data.account
//!   {accountBalanceRv, totalUsedBalanceRv}.
//! - UTA — Query asset GET /uta-biz/assets?currency= (#query-asset, currency обязателен) — {balanceRv,
//!   frozenBalanceAsCollateralRv}.
//! - Фьючерсы coin-M (GET /accounts/accountPositions) — суммы «…Ev» в масштабе valueScale монеты расчёта из справочника
//!   контрактов (не спота) — не читаются, строка с причиной. Earn — только поток wm.subscribe, REST нет.
//!
//! Ответ «счёт не открыт» (урок 27) — кодов в документации нет: ошибка такого счёта идёт в его error (проверить).

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{AccountBalances, Balance};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::rest::Params;
use super::symbols::Symbols;
use super::{account, num, rest};

/// Что делает: читает все счета. Возвращает: счета по порядку. Ошибки: только если не прочитан ни один счёт.
pub async fn read(ctx: &Ctx, symbols: &Symbols) -> GatewayResult<Vec<AccountBalances>> {
    let coins = ctx.list("venue.futures_balance_currencies");
    let (spot, margin, futures, uta) = tokio::join!(
        account::balances(ctx, symbols, None),
        get(ctx, "/margin-trade/wallets", vec![]),
        per_coin(ctx, &coins, "/g-accounts/accountPositions", futures_row),
        per_coin(ctx, &coins, "/uta-biz/assets", uta_row),
    );
    let spot_err = spot.as_ref().err().cloned();
    let mut out = vec![
        done("Спот", spot),
        done("Маржа", margin.and_then(|d| margin_rows(ctx, &d))),
        done("Фьючерсы USDT-M", futures),
        done("UTA (единый счёт)", uta),
        failed(
            "Фьючерсы coin-M",
            "не читается: масштаб сумм — из справочника контрактов, бот читает только спот".into(),
        ),
        failed(
            "Earn",
            "запроса баланса по REST у Phemex нет (только поток wm.subscribe)".into(),
        ),
    ];
    for a in &mut out {
        a.balances
            .retain(|b| !b.free.is_zero() || !b.locked.is_zero());
    }
    if out.iter().all(|a| a.error.is_some()) {
        return Err(spot_err.unwrap_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                "Phemex: не прочитан ни один счёт",
            ))
        }));
    }
    Ok(out)
}

/// Подписанный GET. Ошибки: как rest::signed.
async fn get(ctx: &Ctx, path: &str, q: Params) -> GatewayResult<Value> {
    rest::signed(ctx, Method::GET, path, q, None, Op::Other).await
}

/// Счёт по монетам списка: запрос на монету, строка из ответа. Ошибки: список не задан, первая ошибка запроса.
async fn per_coin(
    ctx: &Ctx,
    coins: &GatewayResult<Vec<String>>,
    path: &str,
    row: fn(&Ctx, &str, &Value) -> GatewayResult<Balance>,
) -> GatewayResult<Vec<Balance>> {
    let coins = coins.as_ref().map_err(|e| e.clone())?;
    let mut out = Vec::with_capacity(coins.len());
    for c in coins {
        let d = get(ctx, path, vec![("currency", c.clone())]).await?;
        out.push(row(ctx, c, &d)?);
    }
    Ok(out)
}

/// Строка USDT-M: свободно = accountBalanceRv − totalUsedBalanceRv, занято = totalUsedBalanceRv. Ошибки: BadResponse.
fn futures_row(ctx: &Ctx, coin: &str, d: &Value) -> GatewayResult<Balance> {
    let acc = d
        .get("account")
        .ok_or_else(|| super::errors::bad("нет account", &d.to_string()))?;
    let total = num::dec(acc, "accountBalanceRv")?;
    let used = num::dec_opt(acc, "totalUsedBalanceRv")?.unwrap_or_default();
    Ok(row(ctx, coin, total - used, used))
}

/// Строка UTA: свободно = balanceRv − frozenBalanceAsCollateralRv. Ошибки: BadResponse.
fn uta_row(ctx: &Ctx, coin: &str, d: &Value) -> GatewayResult<Balance> {
    let total = num::dec(d, "balanceRv")?;
    let frozen = num::dec_opt(d, "frozenBalanceAsCollateralRv")?.unwrap_or_default();
    Ok(row(ctx, coin, total - frozen, frozen))
}

/// Строки маржи: свободно = balanceRq − заморожено (lockedTradingBalanceRq + lockedWithdrawRq). Ошибки: BadResponse.
fn margin_rows(ctx: &Ctx, d: &Value) -> GatewayResult<Vec<Balance>> {
    let list = d
        .as_array()
        .ok_or_else(|| super::errors::bad("margin wallets: data не массив", &d.to_string()))?;
    list.iter()
        .map(|b| {
            let z = |f: &str| num::dec_opt(b, f).map(Option::unwrap_or_default);
            let lock = z("lockedTradingBalanceRq")? + z("lockedWithdrawRq")?;
            Ok(row(
                ctx,
                &num::text(b, "currency")?,
                z("balanceRq")? - lock,
                lock,
            ))
        })
        .collect()
}

/// Строка баланса. Ошибок нет.
fn row(ctx: &Ctx, coin: &str, free: Decimal, locked: Decimal) -> Balance {
    Balance {
        venue: ctx.venue(),
        asset: coin.to_string(),
        free,
        locked,
    }
}

/// Счёт панели из результата. Ошибок нет.
fn done(name: &str, r: GatewayResult<Vec<Balance>>) -> AccountBalances {
    match r {
        Ok(balances) => AccountBalances {
            account: name.to_string(),
            balances,
            error: None,
            note: None,
        },
        Err(e) => failed(name, e.message),
    }
}

/// Счёт, который не прочитан. Ошибок нет.
fn failed(name: &str, reason: String) -> AccountBalances {
    AccountBalances {
        account: name.to_string(),
        balances: Vec::new(),
        error: Some(reason),
        note: None,
    }
}
