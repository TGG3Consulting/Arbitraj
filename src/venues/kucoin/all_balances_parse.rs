//! Файл: venues/kucoin/all_balances_parse.rs
//! Что делает: разбор ответов KuCoin о балансах счетов в формат ядра (решение 144, панель «Балансы»): чистые функции
//! Value → AccountBalances, без сети — их можно проверять на записанных ответах.
//! Кто вызывает: all_balances.rs.
//! От чего зависит: num.rs, core/types (Balance, AccountBalances, VenueId).
//! Настройки: нет.
//!
//! Поля — по спецификации KuCoin (github.com/Kucoin/kucoin-universal-sdk, spec/rest/api, коммит c2f39bd):
//! - Get Account List - Spot (https://www.kucoin.com/docs-new/api-3470125): data[] — currency, type (main / trade /
//!   option), available («Funds available to withdraw or trade»), holds («Funds on hold»).
//! - Get Account - Cross Margin (api-3470127): data.accounts[] — currency, available, hold, liability;
//!   data.totalAssetOfQuoteCurrency, totalLiabilityOfQuoteCurrency, debtRatio, status.
//! - Get Account - Isolated Margin (api-3470128): data.assets[] — symbol, status, debtRatio, baseAsset / quoteAsset
//!   (currency, available, hold, liability).
//! - Get sub-account List - Futures Balance (V2) (api-3470134): data.accounts[] — accountName («main account is
//!   main»), availableBalance, positionMargin, orderMargin, frozenFunds, accountEquity, unrealisedPNL, currency.
//! - Get Account Holding (Earn, api-3470273): data.items[] — currency, holdAmount, redeemingAmount; data.totalPage.

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{AccountBalances, Balance, VenueId};
use crate::gateway::errors::GatewayResult;

use super::errors::bad;
use super::num;

/// Счёт без ошибки. Принимает: название, строки (нулевые отбрасываются), сведения. Ошибок нет.
pub fn account(name: &str, rows: Vec<Balance>, note: Option<String>) -> AccountBalances {
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

/// Счёт, который не прочитан. Принимает: название, причина. Ошибок нет.
pub fn failed(name: &str, reason: String) -> AccountBalances {
    AccountBalances {
        account: name.to_string(),
        balances: Vec::new(),
        error: Some(reason),
        note: None,
    }
}

/// Строки счёта типа ty из ответа Get Account List - Spot. Принимает: площадка, data, тип (main / trade / option).
/// Возвращает: строки (free = available, locked = holds). Ошибки: BadResponse — data не массив или нет полей.
pub fn spot_rows(venue: &VenueId, data: &Value, ty: &str) -> GatewayResult<Vec<Balance>> {
    let list = data
        .as_array()
        .ok_or_else(|| bad("accounts: data не массив", &data.to_string()))?;
    list.iter()
        .filter(|b| num::text_opt(b, "type").as_deref() == Some(ty))
        .map(|b| row(venue, num::text(b, "currency")?, b, "available", "holds"))
        .collect()
}

/// Кросс-маржа. Принимает: площадка, название, data. Возвращает: счёт (free = available, locked = hold), в сведениях —
/// активы и долг в котируемой монете, debtRatio, status и долги по монетам. Ошибки: BadResponse.
pub fn cross_margin(venue: &VenueId, name: &str, data: &Value) -> GatewayResult<AccountBalances> {
    let mut rows = Vec::new();
    let mut debts = Vec::new();
    for a in num::arr(data, "accounts")? {
        let cur = num::text(a, "currency")?;
        debt(&mut debts, &cur, a)?;
        rows.push(row(venue, cur, a, "available", "hold")?);
    }
    let note = format!(
        "активы {} / долг {} (в котируемой монете), debtRatio {}, статус {}{}",
        txt(data, "totalAssetOfQuoteCurrency"),
        txt(data, "totalLiabilityOfQuoteCurrency"),
        txt(data, "debtRatio"),
        txt(data, "status"),
        debts_note(&debts)
    );
    Ok(account(name, rows, Some(note)))
}

/// Изолированная маржа. Принимает: площадка, название, data. Возвращает: счёт; актив строки — «монета (пара)».
/// Ошибки: BadResponse.
pub fn isolated(venue: &VenueId, name: &str, data: &Value) -> GatewayResult<AccountBalances> {
    let mut rows = Vec::new();
    let mut debts = Vec::new();
    for p in num::arr(data, "assets")? {
        let sym = num::text(p, "symbol")?;
        for side in ["baseAsset", "quoteAsset"] {
            let a = p
                .get(side)
                .ok_or_else(|| bad(&format!("нет поля {side}"), &p.to_string()))?;
            let label = format!("{} ({sym})", num::text(a, "currency")?);
            debt(&mut debts, &label, a)?;
            rows.push(row(venue, label, a, "available", "hold")?);
        }
    }
    let note = format!(
        "активы {} / долг {} (в котируемой монете){}",
        txt(data, "totalAssetOfQuoteCurrency"),
        txt(data, "totalLiabilityOfQuoteCurrency"),
        debts_note(&debts)
    );
    Ok(account(name, rows, Some(note)))
}

/// Фьючерсы одной монеты маржи: строка основного счёта (accountName = main). Принимает: площадка, data.
/// Возвращает: (строка: free = availableBalance, locked = positionMargin + orderMargin + frozenFunds; сведения — эквити
/// и нереализованный PnL); None — основного счёта в ответе нет. Ошибки: BadResponse.
pub fn futures_main(venue: &VenueId, data: &Value) -> GatewayResult<Option<(Balance, String)>> {
    let Some(a) = num::arr(data, "accounts")?
        .iter()
        .find(|a| num::text_opt(a, "accountName").as_deref() == Some("main"))
    else {
        return Ok(None);
    };
    let cur = num::text(a, "currency")?;
    let locked =
        num::dec(a, "positionMargin")? + num::dec(a, "orderMargin")? + num::dec(a, "frozenFunds")?;
    let note = format!(
        "{cur}: эквити {}, нереализованный PnL {}",
        num::dec(a, "accountEquity")?,
        num::dec(a, "unrealisedPNL")?
    );
    let b = Balance {
        venue: venue.clone(),
        asset: cur,
        free: num::dec(a, "availableBalance")?,
        locked,
    };
    Ok(Some((b, note)))
}

/// Earn: вложения по монетам (сумма holdAmount; всё в продуктах — locked, свободного нет). Принимает: площадка,
/// название, data. Возвращает: счёт; в сведениях — суммы в погашении и неполная выборка. Ошибки: BadResponse.
pub fn earn(venue: &VenueId, name: &str, data: &Value) -> GatewayResult<AccountBalances> {
    let mut held: Vec<(String, Decimal, Decimal)> = Vec::new();
    for it in num::arr(data, "items")? {
        let cur = num::text(it, "currency")?;
        let (h, r) = (
            num::dec(it, "holdAmount")?,
            num::dec(it, "redeemingAmount")?,
        );
        match held.iter_mut().find(|x| x.0 == cur) {
            Some(x) => (x.1, x.2) = (x.1 + h, x.2 + r),
            None => held.push((cur, h, r)),
        }
    }
    let mut notes: Vec<String> = held
        .iter()
        .filter(|x| !x.2.is_zero())
        .map(|x| format!("в погашении {} {}", x.2, x.0))
        .collect();
    let pages = num::int(data, "totalPage").unwrap_or(1);
    if pages > 1 {
        notes.push(format!("показана 1-я страница из {pages}"));
    }
    let rows = held
        .into_iter()
        .map(|(asset, h, _)| Balance {
            venue: venue.clone(),
            asset,
            free: Decimal::ZERO,
            locked: h,
        })
        .collect();
    let note = (!notes.is_empty()).then(|| notes.join("; "));
    Ok(account(name, rows, note))
}

/// Строка баланса из полей free / locked. Ошибки: BadResponse — нет поля или не число.
fn row(venue: &VenueId, asset: String, v: &Value, f: &str, l: &str) -> GatewayResult<Balance> {
    let (free, locked) = (num::dec(v, f)?, num::dec(v, l)?);
    let venue = venue.clone();
    Ok(Balance {
        venue,
        asset,
        free,
        locked,
    })
}

/// Ненулевой долг (liability) — в список «монета сумма». Ошибки: BadResponse — liability не число.
fn debt(out: &mut Vec<String>, label: &str, v: &Value) -> GatewayResult<()> {
    if let Some(d) = num::dec_opt(v, "liability")?.filter(|d| !d.is_zero()) {
        out.push(format!("{label} {d}"));
    }
    Ok(())
}

/// Хвост сведений о долгах. Ошибок нет.
fn debts_note(debts: &[String]) -> String {
    let tail = (!debts.is_empty()).then(|| format!("; долг: {}", debts.join(", ")));
    tail.unwrap_or_default()
}

/// Поле как текст для сведений («—», если нет). Ошибок нет.
fn txt(v: &Value, field: &str) -> String {
    num::text_opt(v, field).unwrap_or_else(|| "—".into())
}
