//! Файл: venues/mexc/account.rs
//! Что делает: балансы спот-счёта MEXC и кнопка «Проверить подключение»: ключ принят и действует, есть нужные права,
//! совпадает ли привязка к IP с настройками, сколько дней осталось ключу.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.api_ip_bound, venue.api_ip [mexc].
//!
//! Официальная документация:
//! - GET /api/v3/account (SPOT_ACCOUNT_READ, вес 10) https://www.mexc.com/api-docs/spot-v3/spot-account-trade/account-information
//!   — balances[]: asset, free, locked; canTrade, canWithdraw.
//! - GET /api/v3/apiKeyInfo (SPOT_ACCOUNT_READ, вес 1, параметр accessKey) https://www.mexc.com/api-docs/spot-v3/spot-account-trade/query-api-key-info
//!   — status (VALID, DELETE, FROZEN), permissions — строка через запятую, ipWhiteList — строка (формат списка в
//!   документации не указан: делим по запятой, точке с запятой и пробелам), remainingValidity — дни (−999 бессрочно,
//!   0 истёк). Ключ без привязки к IP живёт 90 дней.

use reqwest::Method;

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Права, без которых бот не работает (имена из документации MEXC).
const REQUIRED: [&str; 4] = [
    "SPOT_ACCOUNT_READ",
    "SPOT_DEAL_WRITE",
    "SPOT_WITHDRAW_READ",
    "SPOT_WITHDRAW_WRITE",
];

/// Балансы спот-счёта. Ошибки: ключи, связь, ответ MEXC.
pub async fn balances(ctx: &Ctx) -> GatewayResult<Vec<Balance>> {
    let data = rest::signed(ctx, Method::GET, "/api/v3/account", vec![], Op::Other).await?;
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
    let key = match ctx.keys() {
        Ok((k, _)) => k.to_string(),
        Err(e) => return fail(e.message),
    };
    let p = vec![("accessKey", key)];
    let info = match rest::signed(ctx, Method::GET, "/api/v3/apiKeyInfo", p, Op::Other).await {
        Ok(d) => d,
        Err(e) => return fail(e.message),
    };
    let status = num::text_opt(&info, "status").unwrap_or_default();
    if status != "VALID" {
        return fail(format!("Ключ MEXC не действует: status = {status}"));
    }
    let days = num::int(&info, "remainingValidity").ok();
    if days == Some(0) {
        return fail("Срок ключа MEXC истёк (remainingValidity = 0) — создайте новый".into());
    }
    let perms: Vec<String> = split(&num::text_opt(&info, "permissions").unwrap_or_default());
    let missing: Vec<&str> = REQUIRED
        .iter()
        .copied()
        .filter(|p| !perms.iter().any(|x| x == p))
        .collect();
    if !missing.is_empty() {
        return fail(format!(
            "У ключа нет прав: {} — включите их в MEXC → API Management",
            missing.join(", ")
        ));
    }
    let ips = split(&num::text_opt(&info, "ipWhiteList").unwrap_or_default());
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
                "Привязка к IP включена, но IP {ip} нет в белом списке ключа на MEXC (там: {})",
                list(&ips)
            ));
        }
        format!("ключ привязан к IP {ip}")
    } else {
        format!("привязка к IP не требуется (на MEXC: {})", list(&ips))
    };
    let term = match days {
        Some(-999) => "ключ бессрочный".to_string(),
        Some(d) => format!("ключу осталось {d} дн."),
        None => "срок ключа не сообщён".to_string(),
    };
    ConnectionCheck {
        ok: true,
        details: format!(
            "Подключено: права есть ({}), {ip_note}, {term}",
            REQUIRED.join(", ")
        ),
    }
}

/// Строка списка → элементы (разделители: запятая, точка с запятой, пробелы). Ошибок нет.
fn split(s: &str) -> Vec<String> {
    s.split([',', ';', ' '])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
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
