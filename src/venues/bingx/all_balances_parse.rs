//! Файл: venues/bingx/all_balances_parse.rs
//! Что делает: чистый разбор ответов BingX о балансах счетов (решение 144) — JSON (поле data ответа) → строки
//! и сведения счёта. Сети и настроек нет: только Value → данные, чтобы разбор проверялся на записанных ответах.
//! Кто вызывает: all_balances.rs.
//! От чего зависит: num.rs, core/types (Balance).
//! Настройки: нет.
//!
//! Официальная документация (BingX api-ai-skills — официальные справочники BingX, и docs-v3):
//! - Спот и фонд: data.balances[] {asset, free, locked} —
//!   https://github.com/BingX-API/api-ai-skills/blob/main/skills/spot-account/api-reference.md
//!   («Query Spot Account Balance», «Query Fund Account Balance»); docs-v3 «Query Fund Account Assets» (пример ответа).
//! - Фьючерсы USDT-M: data[] {asset, balance, equity, unrealizedProfit, realisedProfit, availableMargin, usedMargin,
//!   freezedMargin} — https://github.com/BingX-API/api-ai-skills/blob/main/skills/swap-account/api-reference.md
//!   (docs-v3: Swap → Account Endpoints → Query account data).
//! - Фьючерсы COIN-M: data[] {asset, balance, equity, unrealizedProfit, availableMargin, usedMargin, freezedMargin} —
//!   https://github.com/BingX-API/api-ai-skills/blob/main/skills/cswap-trade/api-reference.md (docs-v3: Coin-M Futures
//!   → Query Account Assets).
//! - Стандартные фьючерсы: {asset, balance, crossWalletBalance, crossUnPnl, availableBalance, …} —
//!   https://github.com/BingX-API/api-ai-skills/blob/main/skills/standard-trade/api-reference.md («Query Standard
//!   Contract Balance»); массив это или объект — в документации не сказано, принимаются оба.
//! - Обзор всех счетов: {accountType, usdtBalance}; в api-ai-skills data — массив, в примере docs-v3 —
//!   {result:[…], pageId, total}: принимаются оба —
//!   https://github.com/BingX-API/api-ai-skills/blob/main/skills/fund-account/api-reference.md («Asset Overview»).

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Balance, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::{errors, num};

/// Что делает: строки счёта формата {balances:[{asset, free, locked}]} (спот, фонд).
/// Принимает: площадку, data ответа. Возвращает: только ненулевые строки.
/// Ошибки: BadResponse — нет массива balances или поля строки не те.
pub fn free_locked(venue: &VenueId, data: &Value) -> GatewayResult<Vec<Balance>> {
    let mut out = Vec::new();
    // Пустой фонд: balances = null — счёт пуст, не сбой. Поля balances нет вовсе — ошибка с тем, что пришло (живой
    // ответ 04.11 на /openApi/fund/v1/account/balance — «нет массива balances»; документация docs-v3 — data.balances,
    // api-ai-skills — data.balance (object): форму проверить по тексту ошибки).
    match data.get("balances") {
        Some(Value::Null) => return Ok(out),
        Some(_) => {}
        None => {
            let shown: String = data.to_string().chars().take(400).collect();
            return Err(GatewayError::new(
                ErrorCode::BadResponse,
                None,
                format!("BingX: нет массива balances, пришло: {shown}"),
            ));
        }
    }
    for b in num::arr(data, "balances")? {
        push(
            &mut out,
            venue,
            num::text(b, "asset")?,
            num::dec(b, "free")?,
            num::dec(b, "locked")?,
        );
    }
    Ok(out)
}

/// Что делает: строки фьючерсного счёта USDT-M или COIN-M: free — availableMargin, locked — usedMargin + freezedMargin
/// (поля документации, без своих формул). Сведения — по каждой монете с ненулевым balance или equity: balance,
/// equity, unrealizedProfit и, если есть, realisedProfit.
/// Принимает: площадку, data ответа (массив). Возвращает: (строки, сведения или None).
/// Ошибки: BadResponse — не массив или поля не числа.
pub fn margin(venue: &VenueId, data: &Value) -> GatewayResult<(Vec<Balance>, Option<String>)> {
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    for a in list(data)? {
        let asset = num::text(a, "asset")?;
        let used = num::dec_opt(a, "usedMargin")?.unwrap_or_default();
        let frozen = num::dec_opt(a, "freezedMargin")?.unwrap_or_default();
        push(
            &mut rows,
            venue,
            asset.clone(),
            num::dec(a, "availableMargin")?,
            used + frozen,
        );
        let balance = num::dec(a, "balance")?;
        let equity = num::dec(a, "equity")?;
        if balance.is_zero() && equity.is_zero() {
            continue;
        }
        let mut n = format!(
            "{asset}: баланс {}, эквити {}, нереализованный PnL {}",
            balance.normalize(),
            equity.normalize(),
            num::dec(a, "unrealizedProfit")?.normalize()
        );
        if let Some(r) = num::dec_opt(a, "realisedProfit")? {
            n.push_str(&format!(", реализованный PnL {}", r.normalize()));
        }
        notes.push(n);
    }
    Ok((rows, joined(notes)))
}

/// Что делает: строки стандартных фьючерсов: free — availableBalance, locked — 0 (поля «в марже» в документации нет).
/// Сведения — balance, crossWalletBalance, crossUnPnl по монетам с ненулевым balance.
/// Принимает: площадку, data ответа (массив или объект). Возвращает: (строки, сведения или None).
/// Ошибки: BadResponse — поля не числа.
pub fn standard(venue: &VenueId, data: &Value) -> GatewayResult<(Vec<Balance>, Option<String>)> {
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    for a in list(data)? {
        let asset = num::text(a, "asset")?;
        let balance = num::dec(a, "balance")?;
        push(
            &mut rows,
            venue,
            asset.clone(),
            num::dec(a, "availableBalance")?,
            Decimal::ZERO,
        );
        if !balance.is_zero() {
            notes.push(format!(
                "{asset}: всего {}, кросс-кошелёк {}, нереализованный PnL {}",
                balance.normalize(),
                num::dec_opt(a, "crossWalletBalance")?
                    .unwrap_or_default()
                    .normalize(),
                num::dec_opt(a, "crossUnPnl")?
                    .unwrap_or_default()
                    .normalize()
            ));
        }
    }
    Ok((rows, joined(notes)))
}

/// Что делает: обзор всех счетов — пары (accountType, usdtBalance) как прислал BingX.
/// Принимает: data ответа (массив или {result:[…]}). Возвращает: список пар.
/// Ошибки: BadResponse — нет массива, поля accountType или usdtBalance не число.
pub fn overview(data: &Value) -> GatewayResult<Vec<(String, Decimal)>> {
    let rows = match data.get("result") {
        Some(r) => r,
        None => data,
    };
    list(rows)?
        .iter()
        .map(|r| Ok((num::text(r, "accountType")?, num::dec(r, "usdtBalance")?)))
        .collect()
}

/// Что делает: название счёта по-русски для accountType обзора (значения — из документации «Asset Overview»).
/// Принимает: accountType. Возвращает: название; неизвестный тип — как прислал BingX. Ошибок нет.
pub fn overview_name(kind: &str) -> String {
    match kind {
        "sopt" => "Спот / Фонд (sopt)".into(),
        "stdFutures" => "Стандартные фьючерсы".into(),
        "coinMPerp" => "Фьючерсы COIN-M".into(),
        "USDTMPerp" => "Фьючерсы USDT-M".into(),
        "copyTrading" => "Копитрейдинг".into(),
        "grid" => "Сеточные стратегии (grid)".into(),
        "eran" => "Earn (сбережения)".into(),
        "c2c" => "C2C".into(),
        other => format!("Счёт BingX «{other}»"),
    }
}

/// Массив: сам data, если массив; объект — один элемент. Ошибка BadResponse — иное.
fn list(data: &Value) -> GatewayResult<Vec<&Value>> {
    match data {
        Value::Array(a) => Ok(a.iter().collect()),
        Value::Object(_) => Ok(vec![data]),
        other => Err(errors::bad("ожидался массив счетов", &other.to_string())),
    }
}

/// Добавляет строку, если она не нулевая.
fn push(out: &mut Vec<Balance>, venue: &VenueId, asset: String, free: Decimal, locked: Decimal) {
    if free.is_zero() && locked.is_zero() {
        return;
    }
    out.push(Balance {
        venue: venue.clone(),
        asset,
        free: free.normalize(),
        locked: locked.normalize(),
    });
}

/// Сведения через «; », пусто — None.
fn joined(notes: Vec<String>) -> Option<String> {
    (!notes.is_empty()).then(|| notes.join("; "))
}
