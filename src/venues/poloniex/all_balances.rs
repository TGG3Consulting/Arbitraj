//! Файл: venues/poloniex/all_balances.rs
//! Что делает: балансы ВСЕХ счетов Poloniex для панели «Балансы» (решение 144, урок 24) — только чтение: Спот (по
//! монетам), Маржа (оценка счёта и займы по монетам — в пояснении) и Фьючерсы (отдельный API с отдельными ключами — не читается,
//! сказано в error). Запросы — параллельно; сбой одного счёта — в его error, остальные показываются.
//! Кто вызывает: mod.rs (Venue::account_balances), панель «Балансы» раз в ui.balances_refresh_ms, пока открыта.
//! От чего зависит: rest.rs, account.rs (строки спота), num.rs, ctx.rs.
//! Настройки: как у rest.rs.
//!
//! Официально:
//! - All Account Balances GET /accounts/balances — account.rs (accountType «SPOT is the default and only supported»).
//! - Account Margin GET /margin/accountMargin?accountType=SPOT — totalAccountValue, totalMargin, usedMargin, freeMargin,
//!   maintenanceMargin, marginRatio; Borrow Status GET /margin/borrowStatus — currency, available, borrowed, hold
//!   (https://api-docs.poloniex.com/spot/api/private/margin). Маржа идёт на том же спот-счёте («available … can be
//!   negative when margin is used»): здесь — оценка и займы, не отдельные деньги.
//! - Фьючерсы — отдельный API v3 (https://api-docs.poloniex.com/v3/futures/api/) и отдельные ключи «Futures API Keys»;
//!   боту (только спот) фьючерсный ключ не нужен. Funding / earn-счетов в Spot API нет.
//!
//! Ответ «счёт не открыт» (урок 27) — в документации нет: ошибка маржи — в её error (проверить вживую).

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{AccountBalances, Balance};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::rest::Params;
use super::{account, num, rest};

/// Названия счетов в порядке read.
const NAMES: [&str; 3] = ["Спот", "Маржа (займы на спот-счёте)", "Фьючерсы"];

/// Что делает: читает все счета параллельно. Возвращает: счета по порядку NAMES.
/// Ошибки: ошибка спота — только если не прочитан ни один счёт.
pub async fn read(ctx: &Ctx) -> GatewayResult<Vec<AccountBalances>> {
    let spot_q = vec![("accountType", "SPOT".to_string())];
    let (spot, margin, borrow) = tokio::join!(
        get(ctx, "/accounts/balances", spot_q.clone()),
        get(ctx, "/margin/accountMargin", spot_q),
        get(ctx, "/margin/borrowStatus", vec![]),
    );
    let spot_err = spot.as_ref().err().cloned();
    let spot = spot
        .and_then(|d| account::spot_rows(ctx, &d))
        .map(|r| account_of(NAMES[0], r, None))
        .unwrap_or_else(|e| failed(NAMES[0], e.message));
    let margin = match (margin, borrow) {
        (Ok(m), Ok(b)) => match (summary(&m), loans(&b)) {
            (Ok(n), Ok(l)) => account_of(NAMES[1], vec![], Some(format!("{n}; {l}"))),
            (Err(e), _) | (_, Err(e)) => failed(NAMES[1], e.message),
        },
        (Err(e), _) | (_, Err(e)) => failed(NAMES[1], e.message),
    };
    let futures = failed(
        NAMES[2],
        "Фьючерсы Poloniex — отдельный API v3 с отдельными ключами (Futures API Keys); бот только спот — не читается"
            .into(),
    );
    let out = vec![spot, margin, futures];
    if out.iter().all(|a| a.error.is_some()) {
        return Err(spot_err.unwrap_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                "Poloniex: не прочитан ни один счёт",
            ))
        }));
    }
    Ok(out)
}

/// Подписанный GET. Принимает: путь, параметры. Возвращает: тело. Ошибки: как rest::signed.
async fn get(ctx: &Ctx, path: &str, q: Params) -> GatewayResult<Value> {
    rest::signed(ctx, Method::GET, path, q, None, Op::Other).await
}

/// Оценка маржинального счёта: «стоимость счёта X, использовано маржи Y, свободно Z, marginRatio R».
/// Ошибки: BadResponse — поле не число.
fn summary(m: &Value) -> GatewayResult<String> {
    let d = |f: &str| num::dec_opt(m, f).map(|v| v.unwrap_or_default().normalize());
    Ok(format!(
        "стоимость счёта {} USDT, использовано маржи {}, свободно {}, marginRatio {}",
        d("totalAccountValue")?,
        d("usedMargin")?,
        d("freeMargin")?,
        num::text_opt(m, "marginRatio").unwrap_or_default()
    ))
}

/// Займы по монетам (borrowed ≠ 0): «BTC -10; ETH -1» или «займов нет». Ошибки: BadResponse — ответ не массив.
fn loans(data: &Value) -> GatewayResult<String> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("borrowStatus: ответ не массив", &data.to_string()))?;
    let mut parts = Vec::new();
    for b in list {
        let borrowed = num::dec_opt(b, "borrowed")?.unwrap_or(Decimal::ZERO);
        if !borrowed.is_zero() {
            parts.push(format!(
                "{} {}",
                num::text(b, "currency")?,
                borrowed.normalize()
            ));
        }
    }
    Ok(if parts.is_empty() {
        "займов нет".into()
    } else {
        format!("займы: {}", parts.join("; "))
    })
}

/// Счёт панели: только ненулевые строки. Ошибок нет.
fn account_of(name: &str, rows: Vec<Balance>, note: Option<String>) -> AccountBalances {
    AccountBalances {
        account: name.to_string(),
        balances: rows
            .into_iter()
            .filter(|b| !b.free.is_zero() || !b.locked.is_zero())
            .collect(),
        error: None,
        note,
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
