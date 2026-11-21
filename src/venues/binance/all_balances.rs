//! Файл: venues/binance/all_balances.rs
//! Что делает: балансы всех счетов Binance для панели «Балансы» (решение 144, урок 24) — «Спот», «Фонд» (Funding) и
//! «Все кошельки» (сводка по кошелькам в оценке Binance). Только чтение, на торговлю не влияет. Счета читаются
//! параллельно; ошибка одного — в его поле error, остальные показываются.
//! Кто вызывает: mod.rs (account_balances), раз в ui.balances_refresh_ms, пока панель открыта.
//! От чего зависит: account.rs, rest.rs, num.rs, core/types.
//! Настройки: нет своих.
//!
//! Официально (Wallet SAPI, официальный SDK binance-connector-rust src/wallet/rest_api):
//! - «Спот»: GET /api/v3/account (вес 20) — balances[] asset, free, locked.
//! - «Фонд»: POST /sapi/v1/asset/get-funding-asset (вес 1) — [{asset, free, locked, freeze, withdrawing}]; в заметке
//!   SDK — «Currently supports querying the following business assets: Binance Pay, Binance Card, Binance Gift Card,
//!   Stock Token»: полноту по крипте проверить на живом ответе.
//! - «Все кошельки»: GET /sapi/v1/asset/wallet/balance (вес 60) — [{activate, balance, walletName}], balance в оценке
//!   (по умолчанию BTC) — строки монет нет, показывается примечанием. Маржа, фьючерсы, Earn поштучно — отдельные API;
//!   их суммы видны в этой сводке.

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{AccountBalances, Balance, VenueId};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{account, num, rest};

/// Что делает: читает счета параллельно. Возвращает: [«Спот», «Фонд», «Все кошельки»]. Ошибок нет (в error счёта).
pub async fn all(ctx: &Ctx) -> Vec<AccountBalances> {
    let fund_path = "/sapi/v1/asset/get-funding-asset";
    let wallet_path = "/sapi/v1/asset/wallet/balance";
    let (spot, fund, wallets) = tokio::join!(
        account::balances(ctx),
        rest::signed(ctx, Method::POST, fund_path, vec![], Op::Other),
        rest::signed(ctx, Method::GET, wallet_path, vec![], Op::Other),
    );
    let spot = match spot {
        Ok(b) => row("Спот", b, None),
        Err(e) => failed("Спот", e.message),
    };
    let fund = match fund.and_then(|d| parse_funding(&ctx.venue(), &d)) {
        Ok(b) => row("Фонд", b, None),
        Err(e) => failed("Фонд", e.message),
    };
    let wallets = match wallets.and_then(|d| wallets_note(&d)) {
        Ok(note) => row("Все кошельки", Vec::new(), Some(note)),
        Err(e) => failed("Все кошельки", e.message),
    };
    vec![spot, fund, wallets]
}

/// Что делает: разбор get-funding-asset (ненулевые строки: free; locked = locked + freeze + withdrawing).
/// Ошибка BadResponse — не массив, нет asset или поле не число.
pub fn parse_funding(venue: &VenueId, data: &Value) -> GatewayResult<Vec<Balance>> {
    let rows = data
        .as_array()
        .ok_or_else(|| super::errors::bad("get-funding-asset не массив", &data.to_string()))?;
    let mut out = Vec::new();
    for r in rows {
        let z = |f: &str| num::dec_opt(r, f).map(|v| v.unwrap_or(Decimal::ZERO));
        let free = z("free")?;
        let locked = z("locked")? + z("freeze")? + z("withdrawing")?;
        if !free.is_zero() || !locked.is_zero() {
            out.push(Balance {
                venue: venue.clone(),
                asset: num::text(r, "asset")?,
                free,
                locked,
            });
        }
    }
    Ok(out)
}

/// Что делает: сводка wallet/balance текстом «Spot: 0.1; Funding: 0 …» (ненулевые; оценка — в единицах ответа).
/// Ошибка BadResponse — не массив или balance не число.
pub fn wallets_note(data: &Value) -> GatewayResult<String> {
    let rows = data
        .as_array()
        .ok_or_else(|| super::errors::bad("wallet/balance не массив", &data.to_string()))?;
    let mut parts = Vec::new();
    for r in rows {
        let b = num::dec_opt(r, "balance")?.unwrap_or(Decimal::ZERO);
        if !b.is_zero() {
            let name = num::text_opt(r, "walletName").unwrap_or_else(|| "?".into());
            parts.push(format!("{name}: {b}"));
        }
    }
    Ok(if parts.is_empty() {
        "все кошельки пусты (оценка Binance, по умолчанию в BTC)".into()
    } else {
        format!("оценка Binance (по умолчанию в BTC): {}", parts.join("; "))
    })
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
