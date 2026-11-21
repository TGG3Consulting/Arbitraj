//! Файл: venues/mexc/all_balances.rs
//! Что делает: балансы всех счетов MEXC для панели «Балансы» (решение 144) — «Спот» и «Фьючерсы». Только чтение.
//! Счета читаются параллельно; ошибка одного счёта — в его поле error, остальные не страдают.
//! Кто вызывает: mod.rs (контракт account_balances), раз в ui.balances_refresh_ms (~3 с), пока панель открыта.
//! От чего зависит: account.rs (спот), futures_rest.rs (фьючерсы), num.rs, core/types (AccountBalances, Balance).
//! Настройки: venue.futures_rest_url [mexc] (без неё счёт «Фьючерсы» показывает ошибку), плюс настройки rest.rs.
//!
//! Какие счета есть — по официальной документации MEXC:
//! - Спот v3 (https://www.mexc.com/api-docs/spot-v3/introduction): «User Universal Transfer» знает только счета
//!   "SPOT" и "FUTURES"; эндпоинтов маржи, Earn/сбережений и фондового счёта в документации нет — их не читаем.
//! - «Спот»: GET /api/v3/account (SPOT_ACCOUNT_READ, Weight(IP) 10, «rate limit: 2 times/s»)
//!   https://www.mexc.com/api-docs/spot-v3/spot-account-trade/account-information — balances[]: asset, free, locked.
//! - «Фьючерсы»: GET /api/v1/private/account/assets (Contract API v1, «Get all informations of user's asset»,
//!   «Required permissions: Trade reading permission», «Rate limit: 20 times/2 seconds») — data[]: currency,
//!   positionMargin, frozenBalance, availableBalance, cashBalance, equity, unrealized. Подпись — futures_rest.rs.
//!
//! Лимиты при опросе раз в 3 с: спот — 1 запрос /api/v3/account = вес 10 за 3 с ≈ 33 веса за 10 с из корзины IP
//! (README: 300/10 с → ~11 %; исходник документации: 500/10 с на эндпоинт → ~7 %) и 0,33 запроса/с при пределе
//! эндпоинта 2/с. Фьючерсы — 1 запрос за 3 с ≈ 0,67 из 20 за 2 с (~3 %), другой домен — спот-лимит не трогает.

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{AccountBalances, Balance, VenueId};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::{account, futures_rest, num};

/// Путь «Get all informations of user's asset» фьючерсного API.
const FUTURES_ASSETS: &str = "/api/v1/private/account/assets";

/// Что делает: читает все счета MEXC параллельно.
/// Принимает: окружение. Возвращает: [«Спот», «Фьючерсы»] — у каждого строки или error с причиной.
/// Ошибок нет: сбой счёта уходит в его error.
pub async fn all(ctx: &Ctx) -> Vec<AccountBalances> {
    let (spot, fut) = tokio::join!(
        account::balances(ctx),
        futures_rest::get(ctx, FUTURES_ASSETS)
    );
    let spot = match spot {
        Ok(b) => account_ok("Спот", nonzero(b), None),
        Err(e) => account_err("Спот", e.message),
    };
    let fut = match fut.and_then(|d| parse_futures(&ctx.venue(), &d)) {
        Ok((b, note)) => account_ok("Фьючерсы", b, note),
        Err(e) => account_err("Фьючерсы", e.message),
    };
    vec![spot, fut]
}

/// Что делает: разбор data ответа /api/v1/private/account/assets (чистая функция — для теста на записанном ответе).
/// Принимает: площадку, массив data. Возвращает: ненулевые строки (free = availableBalance, locked = frozenBalance +
/// positionMargin) и примечание с эквити и нереализованным PnL по валютам, где они не нулевые.
/// Ошибки: BadResponse — data не массив, нет currency или числа в нужном поле.
pub fn parse_futures(
    venue: &VenueId,
    data: &Value,
) -> GatewayResult<(Vec<Balance>, Option<String>)> {
    let rows = data.as_array().ok_or_else(|| {
        super::errors::bad("data фьючерсных активов не массив", &data.to_string())
    })?;
    let (mut out, mut notes) = (Vec::new(), Vec::new());
    for r in rows {
        let asset = num::text(r, "currency")?;
        let free = num::dec(r, "availableBalance")?;
        let locked = num::dec(r, "frozenBalance")? + num::dec(r, "positionMargin")?;
        let equity = num::dec_opt(r, "equity")?.unwrap_or(Decimal::ZERO);
        let pnl = num::dec_opt(r, "unrealized")?.unwrap_or(Decimal::ZERO);
        if !equity.is_zero() || !pnl.is_zero() {
            notes.push(format!(
                "{asset}: эквити {equity}, нереализованный PnL {pnl}"
            ));
        }
        if !free.is_zero() || !locked.is_zero() {
            out.push(Balance {
                venue: venue.clone(),
                asset,
                free,
                locked,
            });
        }
    }
    let note = (!notes.is_empty()).then(|| notes.join("; "));
    Ok((out, note))
}

/// Только ненулевые строки. Принимает: строки. Возвращает: строки с free ≠ 0 или locked ≠ 0. Ошибок нет.
fn nonzero(b: Vec<Balance>) -> Vec<Balance> {
    b.into_iter()
        .filter(|x| !x.free.is_zero() || !x.locked.is_zero())
        .collect()
}

/// Счёт прочитан. Принимает: название, строки, примечание. Возвращает: AccountBalances. Ошибок нет.
fn account_ok(name: &str, balances: Vec<Balance>, note: Option<String>) -> AccountBalances {
    AccountBalances {
        account: name.into(),
        balances,
        error: None,
        note,
    }
}

/// Счёт не прочитан. Принимает: название, причину. Возвращает: AccountBalances без строк. Ошибок нет.
fn account_err(name: &str, why: String) -> AccountBalances {
    AccountBalances {
        account: name.into(),
        balances: Vec::new(),
        error: Some(why),
        note: None,
    }
}
