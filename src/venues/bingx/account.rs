//! Файл: venues/bingx/account.rs
//! Что делает: балансы спот-счёта BingX и кнопка «Проверить подключение»: ключ принят,
//! есть ли нужные права, совпадает ли привязка к IP с настройками.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.api_ip_bound, venue.api_ip.
//!
//! Официальная документация:
//! - GET /openApi/spot/v1/account/balance — data.balances[]: asset, free, locked.
//! - GET /openApi/v1/account/apiPermissions — permissions (1 спот-торговля, 2 чтение,
//!   3 фьючерсы, 4 универсальный перевод, 5 вывод, 7 перевод между субсчетами), ipAddresses.

use reqwest::Method;
use serde_json::Value;

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Права, без которых бот не работает: спот-торговля, чтение, вывод (коды из документации).
const REQUIRED: [(i64, &str); 3] = [(1, "спот-торговля"), (2, "чтение"), (5, "вывод")];

/// Балансы спот-счёта. Ошибки: ключи, связь, ответ BingX.
pub async fn balances(ctx: &Ctx) -> GatewayResult<Vec<Balance>> {
    let data = rest::signed(
        ctx,
        Method::GET,
        "/openApi/spot/v1/account/balance",
        vec![],
        Op::Other,
    )
    .await?;
    num::arr(&data, "balances")?
        .iter()
        .map(|b| {
            Ok(Balance {
                venue: ctx.venue(),
                asset: num::text(b, "asset")?,
                free: num::dec(b, "free")?,
                locked: num::dec(b, "locked")?,
            })
        })
        .collect()
}

/// Проверка подключения. Не падает: любая проблема — в details с ok=false.
pub async fn check(ctx: &Ctx) -> ConnectionCheck {
    let data = match rest::signed(
        ctx,
        Method::GET,
        "/openApi/v1/account/apiPermissions",
        vec![],
        Op::Other,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => return fail(e.message),
    };
    let perms: Vec<i64> = data
        .get("permissions")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    let missing: Vec<&str> = REQUIRED
        .iter()
        .filter(|(p, _)| !perms.contains(p))
        .map(|(_, n)| *n)
        .collect();
    // Решение 102: перевод депозита с фондового счёта на спот — нужно право «универсальный перевод» (код 4).
    let to_spot = ctx.flag("venue.deposit_to_spot").unwrap_or(false);
    let mut missing = missing;
    if to_spot && !perms.contains(&4) {
        missing.push("универсальный перевод (нужно для venue.deposit_to_spot)");
    }
    if !missing.is_empty() {
        return fail(format!(
            "У ключа нет прав: {} — включите их в BingX → API Management",
            missing.join(", ")
        ));
    }
    let ips: Vec<String> = data
        .get("ipAddresses")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let bound = match ctx.flag("venue.api_ip_bound") {
        Ok(b) => b,
        Err(e) => return fail(e.message),
    };
    let ip_note = if bound {
        let ip = match ctx.text("venue.api_ip") {
            Ok(ip) => ip,
            Err(e) => return fail(e.message),
        };
        if !ips.contains(&ip) {
            return fail(format!(
                "Привязка к IP включена, но IP {ip} нет в белом списке ключа на BingX (там: {})",
                list(&ips)
            ));
        }
        format!("ключ привязан к IP {ip}")
    } else {
        format!("привязка к IP не требуется (на BingX: {})", list(&ips))
    };
    ConnectionCheck {
        ok: true,
        details: format!(
            "Подключено: права есть (спот-торговля, чтение, вывод{}), {ip_note}",
            if to_spot {
                ", универсальный перевод"
            } else {
                ""
            }
        ),
    }
}

/// Результат «не подключено».
fn fail(details: String) -> ConnectionCheck {
    ConnectionCheck { ok: false, details }
}

/// Список IP для текста.
fn list(ips: &[String]) -> String {
    if ips.is_empty() {
        "без привязки".into()
    } else {
        ips.join(", ")
    }
}
