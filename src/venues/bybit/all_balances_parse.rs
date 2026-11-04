//! Файл: venues/bybit/all_balances_parse.rs
//! Что делает: разбор ответов Bybit V5 о балансах счетов в формат ядра (решение 144, панель «Балансы»): чистые функции
//! Value (поле result ответа) → AccountBalances, без сети — их можно проверять на записанных ответах.
//! Кто вызывает: all_balances.rs.
//! От чего зависит: num.rs, errors.rs, core/types (Balance, AccountBalances, VenueId).
//! Настройки: нет.
//!
//! Поля — по официальной документации Bybit V5 (исходник github.com/bybit-exchange/docs, коммит d5a43d2 от 2026-11-04):
//! - «Get Wallet Balance» https://bybit-exchange.github.io/docs/v5/account/wallet-balance — list[]: totalEquity,
//!   totalAvailableBalance, totalInitialMargin, totalPerpUPL (все — USD); coin[]: coin, walletBalance, spotBorrow, locked
//!   («Locked balance due to the Spot open order»), totalOrderIM, totalPositionIM (в режиме portfolio margin — ""),
//!   bonus. Доступно — формула документации для деривативов «walletBalance - totalPositionIM - totalOrderIM - locked -
//!   bonus» минус spotBorrow («Old walletBalance = New walletBalance - spotBorrow»).
//! - «Get All Coins Balance» https://bybit-exchange.github.io/docs/v5/asset/balance/all-balance — balance[]: coin,
//!   walletBalance («Wallet balance»), transferBalance («Transferable balance»).
//! - «Asset Overview» https://bybit-exchange.github.io/docs/v5/asset/balance/asset-overview — list[]: accountType (enum
//!   assetAccountType), totalEquity, valuationCurrency, coinDetail[] (coin, equity) или categories[] (category, equity,
//!   coinDetail[] с extMap.equityUnit у Alpha farm). Разбивки «доступно / заморожено» нет — только equity монеты.

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{AccountBalances, Balance, VenueId};
use crate::gateway::errors::GatewayResult;

use super::num;

/// Название единого торгового счёта (UNIFIED: спот, маржа и деривативы на одном счёте — «Different Account Modes»).
pub const UNIFIED: &str = "Единый торговый (UTA): спот и деривативы";
/// Название счёта Funding.
pub const FUND: &str = "Фонд (Funding)";
/// Название строки, если Asset Overview не прочитан.
pub const OTHERS: &str = "Прочие счета (Asset Overview)";

/// Названия assetAccountType (enum https://bybit-exchange.github.io/docs/v5/enum#assetaccounttype). «BybitPayLater» —
/// так в примере ответа Asset Overview, в enum — «PayLater»: оба. Неизвестный тип показывается как есть.
#[rustfmt::skip]
const NAMES: [(&str, &str); 12] = [
    ("UnifiedTradingAccount", "Единый торговый (UTA)"), ("Earn", "Earn"), ("TradingBot", "Торговые боты"),
    ("CopyTrading", "Копитрейдинг"), ("CryptoLoans", "Криптозаймы"), ("CryptoLoans_legacy", "Криптозаймы (старые)"),
    ("PayLater", "Pay Later"), ("BybitPayLater", "Pay Later"), ("Launchpool", "Launchpool"), ("TradFi", "TradFi"),
    ("MarginStakedSOL", "Margin Staked SOL"), ("Alpha", "Alpha"),
];

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

/// Что делает: единый торговый счёт из ответа Get Wallet Balance (accountType=UNIFIED).
/// Принимает: площадка, result. Возвращает: счёт; free = walletBalance − spotBorrow − locked − totalOrderIM −
/// totalPositionIM − bonus, locked = locked + totalOrderIM + totalPositionIM; note — эквити, доступно, маржа, PnL (USD).
/// Ошибки: BadResponse — нет list, coin или числа.
pub fn unified(venue: &VenueId, data: &Value) -> GatewayResult<AccountBalances> {
    let (mut rows, mut notes) = (Vec::new(), Vec::new());
    for acc in num::arr(data, "list")? {
        for c in acc
            .get("coin")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let z = |f: &str| num::dec_opt(c, f).map(Option::unwrap_or_default);
            let total = num::dec(c, "walletBalance")?;
            let held = z("locked")? + z("totalOrderIM")? + z("totalPositionIM")?;
            rows.push(Balance {
                venue: venue.clone(),
                asset: num::text(c, "coin")?,
                free: total - z("spotBorrow")? - held - z("bonus")?,
                locked: held,
            });
        }
        let usd = |f: &str, label: &str| num::text_opt(acc, f).map(|v| format!("{label} {v} USD"));
        notes.extend(
            [
                usd("totalEquity", "эквити"),
                usd("totalAvailableBalance", "доступно под маржу"),
                usd("totalInitialMargin", "начальная маржа"),
                usd("totalPerpUPL", "нереализованный PnL"),
            ]
            .into_iter()
            .flatten(),
        );
    }
    Ok(account(UNIFIED, rows, joined(notes)))
}

/// Что делает: счёт Funding из ответа Get All Coins Balance (accountType=FUND).
/// Принимает: площадка, result. Возвращает: счёт; free = transferBalance (переводимо), locked = walletBalance −
/// transferBalance. Ошибки: BadResponse — нет balance или числа.
pub fn fund(venue: &VenueId, data: &Value) -> GatewayResult<AccountBalances> {
    let mut rows = Vec::new();
    for c in num::arr(data, "balance")? {
        let total = num::dec(c, "walletBalance")?;
        let free = num::dec_opt(c, "transferBalance")?.unwrap_or_default();
        rows.push(Balance {
            venue: venue.clone(),
            asset: num::text(c, "coin")?,
            free,
            locked: total - free,
        });
    }
    Ok(account(FUND, rows, None))
}

/// Что делает: прочие счета из ответа Asset Overview — каждый accountType (с categories — каждая категория отдельно).
/// Пропускает FundingAccount и категорию crypto у UnifiedTradingAccount: они читаются подробно (fund, unified).
/// Принимает: площадка, result. Возвращает: счета; строка — equity монеты в locked (средства в продуктах, разбивки нет),
/// note — эквити счёта. Ошибки: BadResponse — нет list, accountType или числа equity.
pub fn overview(venue: &VenueId, data: &Value) -> GatewayResult<Vec<AccountBalances>> {
    let mut out = Vec::new();
    for acc in num::arr(data, "list")? {
        let ty = num::text(acc, "accountType")?;
        if ty == "FundingAccount" {
            continue;
        }
        let name = NAMES
            .iter()
            .find(|(k, _)| *k == ty)
            .map_or(ty.as_str(), |(_, n)| n);
        let cur = num::text_opt(acc, "valuationCurrency").unwrap_or_default();
        if let Some(cats) = acc.get("categories").and_then(Value::as_array) {
            for cat in cats {
                let cn = num::text(cat, "category")?;
                if ty == "UnifiedTradingAccount" && cn == "crypto" {
                    continue;
                }
                let eq = num::text_opt(cat, "equity").map(|e| format!("эквити {e} {cur}"));
                out.push(account(
                    &format!("{name} — {cn}"),
                    coins(venue, cat)?,
                    note(eq),
                ));
            }
        } else {
            let eq = num::text_opt(acc, "totalEquity").map(|e| format!("эквити {e} {cur}"));
            out.push(account(name, coins(venue, acc)?, note(eq)));
        }
    }
    Ok(out)
}

/// Строки coinDetail[] (coin, equity; у Alpha farm — единица equity из extMap.equityUnit в имени актива).
/// Ошибки: BadResponse — нет coin или equity.
fn coins(venue: &VenueId, v: &Value) -> GatewayResult<Vec<Balance>> {
    let mut rows = Vec::new();
    for c in v
        .get("coinDetail")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut asset = num::text(c, "coin")?;
        if let Some(u) = c.get("extMap").and_then(|m| num::text_opt(m, "equityUnit")) {
            asset = format!("{asset} (в {u})");
        }
        rows.push(Balance {
            venue: venue.clone(),
            asset,
            free: Decimal::ZERO,
            locked: num::dec(c, "equity")?,
        });
    }
    Ok(rows)
}

/// Сведения счёта Asset Overview: эквити и пояснение, что суммы — equity без разбивки. Ошибок нет.
fn note(eq: Option<String>) -> Option<String> {
    joined(
        eq.into_iter()
            .chain([
                "суммы — эквити монет (equity), разбивки доступно/заморожено Bybit не даёт"
                    .to_string(),
            ])
            .collect(),
    )
}

/// Части сведений через «; »; пусто — None. Ошибок нет.
fn joined(parts: Vec<String>) -> Option<String> {
    (!parts.is_empty()).then(|| parts.join("; "))
}
