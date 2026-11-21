//! Файл: venues/xt/all_balances.rs
//! Что делает: балансы всех счетов XT.com для панели «Балансы» (решение 144, урок 24) — «Спот», «Фьючерсы USDT-M»,
//! «Фьючерсы Coin-M». Только чтение, на торговлю не влияет. Счета читаются параллельно; ошибка одного — в его поле
//! error, остальные показываются. Маржинальный счёт XT отдаёт только по паре (GET /v4/lever/balance?symbol=) — в панель
//! не входит (строка с примечанием); счёта Earn (FINANCE) в API нет.
//! Кто вызывает: mod.rs (account_balances), раз в ui.balances_refresh_ms, пока панель открыта.
//! От чего зависит: account.rs, futures.rs, num.rs, core/types.
//! Настройки: venue.futures_rest_url [xt] (USDT-M, официально https://fapi.xt.com), venue.futures_coin_rest_url [xt]
//! (Coin-M, https://dapi.xt.com). Не заданы — счёт показывает причину.
//!
//! Официально: спот — GET /v4/balances (account.rs); фьючерсы — GET /future/user/v1/balance/list
//! (https://doc.xt.com/docs/futures/User/GetUserFunds, `_futures_user/getBalances.md`, своя подпись — sign.rs) → result
//! [{coin, walletBalance, availableBalance, openOrderMarginFrozen, isolatedMargin, crossedMargin, bonus, coupon}].
//! Свободно = availableBalance, занято = walletBalance − availableBalance. Ответ «счёт фьючерсов не открыт»
//! (/future/user/v1/account/open) в документации не показан — пока это ошибка счёта (урок 27: проверить на живом).

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{AccountBalances, Balance, VenueId};
use crate::gateway::errors::GatewayResult;

use super::ctx::{Ctx, ticker};
use super::{account, num};

/// Путь балансов фьючерсов (одинаков у USDT-M и Coin-M).
const FUTURES_BALANCES: &str = "/future/user/v1/balance/list";

/// Что делает: читает счета параллельно. Возвращает: [«Спот», «Фьючерсы USDT-M», «Фьючерсы Coin-M», «Маржа»].
/// Ошибок нет (в error счёта).
pub async fn all(ctx: &Ctx) -> Vec<AccountBalances> {
    let (spot, usdt, coin) = tokio::join!(
        account::balances(ctx),
        futures(ctx, "venue.futures_rest_url"),
        futures(ctx, "venue.futures_coin_rest_url"),
    );
    let spot = match spot {
        Ok(b) => row("Спот", b, None),
        Err(e) => failed("Спот", e.message),
    };
    let margin = row(
        "Маржа",
        Vec::new(),
        Some("XT отдаёт маржинальный счёт только по паре (GET /v4/lever/balance?symbol=) — не читается".into()),
    );
    vec![
        spot,
        usdt.into_row("Фьючерсы USDT-M"),
        coin.into_row("Фьючерсы Coin-M"),
        margin,
    ]
}

/// Итог чтения фьючерсного счёта.
struct Futures(Result<Vec<Balance>, String>);

impl Futures {
    /// В строку панели. Ошибок нет.
    fn into_row(self, name: &str) -> AccountBalances {
        match self.0 {
            Ok(b) => row(name, b, None),
            Err(why) => failed(name, why),
        }
    }
}

/// Фьючерсный счёт по адресу из настройки. Ошибок нет (причина — в итоге).
async fn futures(ctx: &Ctx, setting: &str) -> Futures {
    let base = match ctx.text(setting) {
        Ok(b) => b,
        Err(e) => return Futures(Err(e.message)),
    };
    let res = super::futures::futures_get(ctx, &base, FUTURES_BALANCES)
        .await
        .and_then(|d| parse_futures(&ctx.venue(), &d));
    Futures(res.map_err(|e| e.message))
}

/// Что делает: разбор result балансов фьючерсов (ненулевые строки). Ошибка BadResponse — не массив, поле не число.
pub fn parse_futures(venue: &VenueId, data: &Value) -> GatewayResult<Vec<Balance>> {
    let rows = data
        .as_array()
        .ok_or_else(|| super::errors::bad("balance/list не массив", &data.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        let z = |f: &str| num::dec_opt(r, f).map(|v| v.unwrap_or(Decimal::ZERO));
        let wallet = z("walletBalance")?;
        let free = z("availableBalance")?;
        let locked = (wallet - free).max(Decimal::ZERO);
        if !free.is_zero() || !locked.is_zero() {
            out.push(Balance {
                venue: venue.clone(),
                asset: ticker(&num::text(r, "coin")?),
                free,
                locked,
            });
        }
    }
    Ok(out)
}

/// Счёт прочитан. Ошибок нет.
fn row(name: &str, balances: Vec<Balance>, note: Option<String>) -> AccountBalances {
    AccountBalances {
        account: name.into(),
        balances,
        error: None,
        note,
    }
}

/// Счёт не прочитан. Ошибок нет.
fn failed(name: &str, why: String) -> AccountBalances {
    AccountBalances {
        account: name.into(),
        balances: Vec::new(),
        error: Some(why),
        note: None,
    }
}
