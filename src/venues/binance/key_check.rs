//! Файл: venues/binance/key_check.rs
//! Что делает: кнопка «Проверить подключение» Binance: ключ принят, права текущего ключа (урок 29), привязка к IP,
//! аккаунт может торговать и выводить, travel rule для вывода. Не падает: любая проблема — в details с ok = false.
//! Кто вызывает: mod.rs (check_connection).
//! От чего зависит: rest.rs, account.rs, ctx.rs.
//! Настройки: venue.api_ip_bound, venue.api_ip, venue.deposit_to_spot [binance].
//!
//! Официально (Wallet SAPI):
//! - GET /sapi/v1/account/apiRestrictions (вес 1) — права ТЕКУЩЕГО ключа: ipRestrict, enableReading,
//!   enableSpotAndMarginTrading (спот и маржа одним флагом), enableWithdrawals («включение вывода требует ограничения
//!   ключа по IP»), permitsUniversalTransfer, enableInternalTransfer, enableMargin, enableFutures, enableVanillaOptions.
//!   Списка IP ключа в ответе нет — сверяется только флаг ipRestrict.
//! - GET /api/v3/account — canTrade, canWithdraw, canDeposit.
//! - GET /sapi/v1/localentity/questionnaire-requirements (вес 1) — «If it returns anything other than NIL» вывод идёт
//!   через POST /sapi/v1/localentity/withdraw/apply (в адаптере не реализован). Имя поля ответа в доступной документации
//!   не показано: проверяется, есть ли в ответе значение "NIL" (проверить на живом ответе).
//!   Белый список адресов вывода (кабинет): −4035 «адрес не в белом списке», −4036 «новый адрес — вывод через N ч» —
//!   адрес кошелька DEX (сеть BSC) добавить заранее.

use reqwest::Method;
use serde_json::Value;

use crate::gateway::contract::ConnectionCheck;

use super::ctx::Ctx;
use super::errors::Op;
use super::{account, rest};

/// Права ключа, без которых бот не работает (имена — из apiRestrictions).
const REQUIRED: [&str; 3] = [
    "enableReading",
    "enableSpotAndMarginTrading",
    "enableWithdrawals",
];
/// Права, которых боту не нужно (показываются, не блокируют — у Тиграна ключи с полными доступами).
const EXTRA: [&str; 4] = [
    "enableMargin",
    "enableFutures",
    "enableVanillaOptions",
    "enablePortfolioMarginTrading",
];

/// Проверка подключения. Ошибок нет: всё — в ConnectionCheck.
pub async fn check(ctx: &Ctx) -> ConnectionCheck {
    if let Err(e) = ctx.keys() {
        return fail(e.message);
    }
    let path = "/sapi/v1/account/apiRestrictions";
    let r = match rest::signed(ctx, Method::GET, path, vec![], Op::Other).await {
        Ok(v) => v,
        Err(e) => return fail(e.message),
    };
    let on = |n: &str| r.get(n).and_then(Value::as_bool) == Some(true);
    let mut need: Vec<&str> = REQUIRED.iter().copied().filter(|n| !on(n)).collect();
    if ctx.flag("venue.deposit_to_spot").unwrap_or(false) && !on("permitsUniversalTransfer") {
        need.push("permitsUniversalTransfer");
    }
    if !need.is_empty() {
        return fail(format!(
            "У ключа Binance нет прав: {} — включите в API Management (вывод — только с ограничением по IP)",
            need.join(", ")
        ));
    }
    let ip_note = match ctx.flag("venue.api_ip_bound") {
        Ok(true) if !on("ipRestrict") => {
            return fail(
                "venue.api_ip_bound = да, но ключ Binance не ограничен по IP (ipRestrict = false)"
                    .into(),
            );
        }
        Ok(true) => format!(
            "ключ ограничен по IP (настройка venue.api_ip: {})",
            ctx.text("venue.api_ip")
                .unwrap_or_else(|_| "не задана".into())
        ),
        Ok(false) => format!("ipRestrict = {}", on("ipRestrict")),
        Err(e) => return fail(e.message),
    };
    let acc = match account::info(ctx).await {
        Ok(a) => a,
        Err(e) => return fail(e.message),
    };
    let can = |n: &str| acc.get(n).and_then(Value::as_bool) == Some(true);
    let blocked: Vec<&str> = ["canTrade", "canWithdraw", "canDeposit"]
        .into_iter()
        .filter(|n| !can(n))
        .collect();
    if !blocked.is_empty() {
        return fail(format!("Аккаунт Binance: {} = false", blocked.join(", ")));
    }
    let tr = "/sapi/v1/localentity/questionnaire-requirements";
    let travel = match rest::signed(ctx, Method::GET, tr, vec![], Op::Other).await {
        Ok(v) if has_nil(&v) => "travel rule для вывода: NIL (не требуется)".to_string(),
        Ok(v) => {
            return fail(format!(
                "Travel rule: вывод требует localentity/withdraw/apply (не реализован), ответ: {v}"
            ));
        }
        Err(e) => return fail(format!("Travel rule не проверен: {}", e.message)),
    };
    let extra: Vec<&str> = EXTRA.iter().copied().filter(|n| on(n)).collect();
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!("; у ключа есть лишние права: {}", extra.join(", "))
    };
    ConnectionCheck {
        ok: true,
        details: format!(
            "Подключено: права есть ({}), {ip_note}, {travel}{extra}",
            REQUIRED.join(", ")
        ),
    }
}

/// Есть ли в ответе строковое значение "NIL" (на любом уровне). Ошибок нет.
pub fn has_nil(v: &Value) -> bool {
    match v {
        Value::String(s) => s == "NIL",
        Value::Array(a) => a.iter().any(has_nil),
        Value::Object(m) => m.values().any(has_nil),
        _ => false,
    }
}

/// Результат «не подключено». Ошибок нет.
fn fail(details: String) -> ConnectionCheck {
    ConnectionCheck { ok: false, details }
}
