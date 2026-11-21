//! Файл: venues/coinex/all_balances.rs
//! Что делает: балансы ВСЕХ счетов CoinEx для панели «Балансы» (решение 144, урок 24) — только чтение: Спот, Маржа (по
//! рынкам), Фьючерсы, Финансовый (Earn). Запросы — параллельно; сбой одного счёта — в его error, остальные показываются.
//! Кредитный счёт и AMM не показываются (бот их не использует; формат — отдельные страницы).
//! Кто вызывает: mod.rs (Venue::account_balances), панель «Балансы» раз в ui.balances_refresh_ms, пока открыта.
//! От чего зависит: rest.rs, account.rs (строки спота), num.rs, ctx.rs.
//! Настройки: как у rest.rs.
//!
//! Официально (раздел Assets → Balance, https://docs.coinex.com/api/v2/assets/balance/http/get-spot-balance и соседние):
//! - Спот GET /assets/spot/balance — account.rs.
//! - Маржа GET /assets/margin/balance — data[] {margin_account (рынок), base_ccy, quote_ccy, available {base_ccy,
//!   quote_ccy}, frozen {…}, repaid, interest} (get-marigin-balance).
//! - Фьючерсы GET /assets/futures/balance — data[] {ccy, account_balance, frozen, available (deprecated), …}: строка —
//!   account_balance («Cross margin balance plus isolated margin balance»), frozen — в «заморожено».
//! - Финансовый GET /assets/financial/balance — data[] {ccy, available, frozen}.
//!
//! «Счёт не открыт» (урок 27) — кода в документации нет: ошибка такого счёта — в его error (проверить вживую).

use reqwest::Method;
use serde_json::Value;

use crate::core::types::{AccountBalances, Balance};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{account, num, rest};

/// Названия счетов в порядке read.
const NAMES: [&str; 4] = ["Спот", "Маржа", "Фьючерсы", "Финансовый (Earn)"];

/// Что делает: читает все счета параллельно. Возвращает: счета по порядку NAMES.
/// Ошибки: ошибка спота — только если не прочитан ни один счёт.
pub async fn read(ctx: &Ctx) -> GatewayResult<Vec<AccountBalances>> {
    let (spot, margin, fut, fin) = tokio::join!(
        get(ctx, "/v2/assets/spot/balance"),
        get(ctx, "/v2/assets/margin/balance"),
        get(ctx, "/v2/assets/futures/balance"),
        get(ctx, "/v2/assets/financial/balance"),
    );
    let spot_err = spot.as_ref().err().cloned();
    let out: Vec<GatewayResult<Vec<Balance>>> = vec![
        spot.and_then(|d| account::spot_rows(ctx, &d)),
        margin.and_then(|d| margin_rows(ctx, &d)),
        fut.and_then(|d| rows(ctx, &d, "account_balance")),
        fin.and_then(|d| rows(ctx, &d, "available")),
    ];
    let out: Vec<AccountBalances> = out
        .into_iter()
        .zip(NAMES)
        .map(|(r, name)| match r {
            Ok(rows) => account_of(name, rows),
            Err(e) => failed(name, e.message),
        })
        .collect();
    if out.iter().all(|a| a.error.is_some()) {
        return Err(spot_err.unwrap_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                "CoinEx: не прочитан ни один счёт",
            ))
        }));
    }
    Ok(out)
}

/// Подписанный GET без параметров. Возвращает: data. Ошибки: как rest::signed.
async fn get(ctx: &Ctx, path: &str) -> GatewayResult<Value> {
    rest::signed(ctx, Method::GET, path, vec![], None, Op::Other).await
}

/// Строки {ccy, <поле свободного>, frozen}. Принимает: data, имя поля «свободно». Ошибки: BadResponse.
fn rows(ctx: &Ctx, data: &Value, free: &str) -> GatewayResult<Vec<Balance>> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("data не массив", &data.to_string()))?;
    list.iter()
        .map(|b| {
            Ok(Balance {
                venue: ctx.venue(),
                asset: num::text(b, "ccy")?.to_ascii_uppercase(),
                free: num::dec_opt(b, free)?.unwrap_or_default(),
                locked: num::dec_opt(b, "frozen")?.unwrap_or_default(),
            })
        })
        .collect()
}

/// Строки маржи: по две на рынок — «МОНЕТА (РЫНОК)» базовой и котируемой. Ошибки: BadResponse.
fn margin_rows(ctx: &Ctx, data: &Value) -> GatewayResult<Vec<Balance>> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("margin/balance: data не массив", &data.to_string()))?;
    let mut out = Vec::new();
    for m in list {
        let market = num::text(m, "margin_account")?;
        for side in ["base_ccy", "quote_ccy"] {
            let part = |k: &str| {
                m.get(k)
                    .map(|o| num::dec_opt(o, side).map(Option::unwrap_or_default))
                    .unwrap_or(Ok(Default::default()))
            };
            out.push(Balance {
                venue: ctx.venue(),
                asset: format!("{} ({market})", num::text(m, side)?),
                free: part("available")?,
                locked: part("frozen")?,
            });
        }
    }
    Ok(out)
}

/// Счёт панели: только ненулевые строки. Ошибок нет.
fn account_of(name: &str, rows: Vec<Balance>) -> AccountBalances {
    AccountBalances {
        account: name.to_string(),
        balances: rows
            .into_iter()
            .filter(|b| !b.free.is_zero() || !b.locked.is_zero())
            .collect(),
        error: None,
        note: None,
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
