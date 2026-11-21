//! Файл: venues/bitget/all_balances.rs
//! Что делает: балансы ВСЕХ счетов Bitget для панели «Балансы» (решение 144, урок 24) — только чтение: Спот, Фонд
//! (Funding/P2P), Боты (спот и фьючерсы) и итог всех счетов в USDT (фьючерсы, маржа, earn — у Classic нет отдельных
//! эндпоинтов балансов по монетам в общих разделах; их оценка — в итоге). Запросы — параллельно; сбой одного счёта — в
//! его error, остальные показываются.
//! Кто вызывает: mod.rs (Venue::account_balances), панель «Балансы» раз в ui.balances_refresh_ms, пока открыта.
//! От чего зависит: rest.rs, account.rs (строки спота), num.rs, ctx.rs.
//! Настройки: как у rest.rs.
//!
//! Официально:
//! - Get Account Assets GET /api/v2/spot/account/assets — account.rs.
//! - Funding Assets GET /api/v2/account/funding-assets («10 times/1s (UID)») — coin, available, frozen, usdtValue;
//!   «Sub-account requests are not allowed» https://www.bitget.com/docs/catalog/classic-common-account/classic-common-account#funding-assets
//! - Bot Assets GET /api/v2/account/bot-assets?accountType=futures|spot — coin, available, equity, bonus, frozen, usdtValue.
//! - Get All Account Balance GET /api/v2/account/all-account-balance («1 times/1s (UID)») — accountType (в примере spot,
//!   futures, funding, earn, bots, margin), usdtBalance. Раз в несколько секунд — в пределах «1 times/1s».
//!
//! Ответ «счёт не открыт» (урок 27) — кодов в документации нет: ошибка такого счёта идёт в его error (проверить вживую).

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
const NAMES: [&str; 5] = [
    "Спот",
    "Фонд (Funding/P2P)",
    "Боты (спот)",
    "Боты (фьючерсы)",
    "Все счета (оценка в USDT)",
];

/// Что делает: читает все счета параллельно. Возвращает: счета по порядку NAMES.
/// Ошибки: ошибка спота — только если не прочитан ни один счёт.
pub async fn read(ctx: &Ctx) -> GatewayResult<Vec<AccountBalances>> {
    let (spot, fund, bot_s, bot_f, all) = tokio::join!(
        get(ctx, "/api/v2/spot/account/assets", vec![]),
        get(ctx, "/api/v2/account/funding-assets", vec![]),
        get(
            ctx,
            "/api/v2/account/bot-assets",
            vec![("accountType", "spot".into())]
        ),
        get(
            ctx,
            "/api/v2/account/bot-assets",
            vec![("accountType", "futures".into())]
        ),
        get(ctx, "/api/v2/account/all-account-balance", vec![]),
    );
    let spot_err = spot.as_ref().err().cloned();
    let out: Vec<GatewayResult<AccountBalances>> = vec![
        spot.and_then(|d| account::spot_rows(ctx, &d))
            .map(|r| account_of(NAMES[0], r, None)),
        fund.and_then(|d| rows(ctx, &d))
            .map(|r| account_of(NAMES[1], r, None)),
        bot_s
            .and_then(|d| rows(ctx, &d))
            .map(|r| account_of(NAMES[2], r, None)),
        bot_f
            .and_then(|d| rows(ctx, &d))
            .map(|r| account_of(NAMES[3], r, None)),
        all.and_then(|d| totals(&d))
            .map(|n| account_of(NAMES[4], vec![], Some(n))),
    ];
    let out: Vec<AccountBalances> = out
        .into_iter()
        .zip(NAMES)
        .map(|(r, name)| r.unwrap_or_else(|e| failed(name, e.message)))
        .collect();
    if out.iter().all(|a| a.error.is_some()) {
        return Err(spot_err.unwrap_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                "Bitget: не прочитан ни один счёт",
            ))
        }));
    }
    Ok(out)
}

/// Подписанный GET. Принимает: путь, параметры. Возвращает: data. Ошибки: как rest::signed.
async fn get(ctx: &Ctx, path: &str, q: Params) -> GatewayResult<Value> {
    rest::signed(ctx, Method::GET, path, q, None, Op::Other).await
}

/// Строки фонда и ботов: free = available, locked = frozen (пусто — 0). Ошибки: BadResponse.
fn rows(ctx: &Ctx, data: &Value) -> GatewayResult<Vec<Balance>> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("data не массив", &data.to_string()))?;
    list.iter()
        .map(|b| {
            Ok(Balance {
                venue: ctx.venue(),
                asset: num::text(b, "coin")?.to_ascii_uppercase(),
                free: num::dec_opt(b, "available")?.unwrap_or_default(),
                locked: num::dec_opt(b, "frozen")?.unwrap_or_default(),
            })
        })
        .collect()
}

/// Итог всех счетов: «spot 12.3 USDT; futures 0 USDT; …». Ошибки: BadResponse.
fn totals(data: &Value) -> GatewayResult<String> {
    let list = data.as_array().ok_or_else(|| {
        super::errors::bad("all-account-balance: data не массив", &data.to_string())
    })?;
    let parts: GatewayResult<Vec<String>> = list
        .iter()
        .map(|x| {
            let usdt: Decimal = num::dec_opt(x, "usdtBalance")?.unwrap_or_default();
            Ok(format!(
                "{} {} USDT",
                num::text(x, "accountType")?,
                usdt.normalize()
            ))
        })
        .collect();
    Ok(parts?.join("; "))
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
