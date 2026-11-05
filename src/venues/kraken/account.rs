//! Файл: venues/kraken/account.rs
//! Что делает: балансы спот-кошелька Kraken (BalanceEx: свободно = balance − hold_trade) и кнопка «Проверить
//! подключение»: ключ принят, права своей записи ключа (урок 29), срок ключа, привязка к IP, доступ к Funding (Beta) и
//! есть ли у котируемой монеты сеть BSC.
//! Кто вызывает: mod.rs (функции контракта), all_balances.rs.
//! От чего зависит: rest.rs, num.rs, methods.rs, ctx.rs.
//! Настройки: venue.api_ip_bound, venue.api_ip, venue.quote_asset, venue.bsc_network [kraken].
//!
//! Официально (OpenAPI spot-rest.yaml):
//! - POST /0/private/BalanceEx — result «актив → {balance, hold_trade, credit, credit_used}»; доступно = balance + credit
//!   − credit_used − hold_trade. Кредит — не свои деньги: бот считает свободным balance − hold_trade. Суффиксы .B, .F, .T
//!   (и .S, .M — только чтение) — другие кошельки (Earn и пр.), на споте не торгуются — в all_balances.rs.
//! - POST /0/private/GetApiKeyInfo (с 11.03.2026) — api_key_name, permissions, ip_allowlist, valid_until, nonce_window,
//!   last_used; пример в спецификации — camelCase, схема — snake_case: читаются оба вида (сверить на живом ответе, урок 2).
//!   Имена прав — из спецификации: query-funds, withdraw-funds, query-open-trades, query-closed-trades, modify-trades,
//!   close-trades, create-ws-token; лишнее и опасное для бота — add-withdraw-address, update-withdraw-address.

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::methods::{self, Dir};
use super::{num, rest};

/// Права, без которых бот не работает (имена permissions GetApiKeyInfo).
const REQUIRED: [&str; 7] = [
    "query-funds",
    "withdraw-funds",
    "query-open-trades",
    "query-closed-trades",
    "modify-trades",
    "close-trades",
    "create-ws-token",
];
/// Права, которые боту не нужны и опасны (добавление адресов вывода).
const RISKY: [&str; 2] = ["add-withdraw-address", "update-withdraw-address"];

/// Балансы: спот (активы без суффикса кошелька) и прочие кошельки отдельно. Ошибки: ключи, связь, Kraken.
pub async fn read(ctx: &Ctx) -> GatewayResult<(Vec<Balance>, Vec<Balance>)> {
    let res = rest::private(ctx, "BalanceEx", vec![], Op::Other).await?;
    parse(&ctx.venue(), &res)
}

/// Спот-балансы. Ошибки: как read.
pub async fn balances(ctx: &Ctx) -> GatewayResult<Vec<Balance>> {
    Ok(read(ctx).await?.0)
}

/// Разбор result BalanceEx: (спот, прочие кошельки с суффиксом — всё в locked). Пустой — пустой счёт (урок 27).
/// Ошибка BadResponse — result не объект или balance не число.
pub fn parse(
    venue: &crate::core::types::VenueId,
    res: &serde_json::Value,
) -> GatewayResult<(Vec<Balance>, Vec<Balance>)> {
    let map = res
        .as_object()
        .ok_or_else(|| super::errors::bad("BalanceEx: result не объект", &res.to_string()))?;
    let (mut spot, mut other) = (vec![], vec![]);
    for (asset, b) in map {
        let total = num::dec(b, "balance")?;
        let hold = num::dec_opt(b, "hold_trade")?.unwrap_or_default();
        if asset.contains('.') {
            other.push(Balance {
                venue: venue.clone(),
                asset: asset.clone(),
                free: Default::default(),
                locked: total,
            });
        } else {
            spot.push(Balance {
                venue: venue.clone(),
                asset: asset.clone(),
                free: total - hold,
                locked: hold,
            });
        }
    }
    Ok((spot, other))
}

/// Проверка подключения. Не падает: любая проблема — в details с ok=false.
pub async fn check(ctx: &Ctx) -> ConnectionCheck {
    match run(ctx).await {
        Ok(details) => ConnectionCheck { ok: true, details },
        Err(details) => ConnectionCheck { ok: false, details },
    }
}

/// Сама проверка. Возвращает: текст «Подключено…». Ошибка — точная причина.
async fn run(ctx: &Ctx) -> Result<String, String> {
    let k = rest::private(ctx, "GetApiKeyInfo", vec![], Op::Other)
        .await
        .map_err(|e| e.message)?;
    let f = |snake: &str, camel: &str| k.get(snake).or_else(|| k.get(camel)).cloned();
    let list = |v: Option<serde_json::Value>| -> Vec<String> {
        v.and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect()
    };
    let Some(perms_raw) = f("permissions", "permissions") else {
        return Err(format!(
            "GetApiKeyInfo без permissions — формат ответа не тот: {k}"
        ));
    };
    let perms = list(Some(perms_raw));
    let missing: Vec<&str> = REQUIRED
        .into_iter()
        .filter(|p| !perms.iter().any(|x| x == p))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "У ключа Kraken нет прав: {} (есть: {})",
            missing.join(", "),
            perms.join(", ")
        ));
    }
    let risky: Vec<&str> = RISKY
        .into_iter()
        .filter(|p| perms.iter().any(|x| x == p))
        .collect();
    let ips = list(f("ip_allowlist", "ipAllowlist"));
    let ip_note = if ctx.flag("venue.api_ip_bound").map_err(|e| e.message)? {
        let ip = ctx.text("venue.api_ip").map_err(|e| e.message)?;
        if !ips.contains(&ip) {
            return Err(format!(
                "Привязка к IP включена, но IP {ip} нет у ключа Kraken (там: {})",
                ips.join(", ")
            ));
        }
        format!("ключ привязан к IP {ip}")
    } else {
        format!(
            "привязка к IP не требуется (у ключа: {})",
            if ips.is_empty() {
                "без привязки".into()
            } else {
                ips.join(", ")
            }
        )
    };
    let s = |v: Option<serde_json::Value>| {
        v.map(|v| v.as_str().map_or(v.to_string(), str::to_string))
            .unwrap_or_else(|| "не указан".into())
    };
    let quote = ctx.text("venue.quote_asset").map_err(|e| e.message)?;
    let bsc = ctx.text("venue.bsc_network").map_err(|e| e.message)?;
    let wd = methods::list(ctx, Dir::Withdraw, Some(&quote))
        .await
        .map_err(|e| format!("Funding (Beta) недоступен: {}", e.message))?;
    let nets: Vec<String> = wd
        .iter()
        .filter_map(|m| {
            m.get("network")
                .and_then(|n| num::text_opt(n, "network_name"))
        })
        .collect();
    let bsc_note = if nets.contains(&bsc) {
        format!("у {quote} есть вывод по сети «{bsc}»")
    } else {
        format!(
            "у {quote} нет вывода по сети «{bsc}» (сети вывода: {}) — проверьте venue.bsc_network",
            nets.join(", ")
        )
    };
    let warn = if risky.is_empty() {
        String::new()
    } else {
        format!(
            "; ВНИМАНИЕ: у ключа лишние права {} — снимите",
            risky.join(", ")
        )
    };
    Ok(format!(
        "Подключено. Ключ «{}», срок {}, окно nonce {}. Права: {}; {ip_note}; {bsc_note}{warn}",
        s(f("api_key_name", "apiKeyName")),
        s(f("valid_until", "validUntil")),
        s(f("nonce_window", "nonceWindow")),
        perms.join(", ")
    ))
}
