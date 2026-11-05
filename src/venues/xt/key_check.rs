//! Файл: venues/xt/key_check.rs
//! Что делает: кнопка «Проверить подключение» XT.com: ключ и подпись приняты (подписанный GET /v4/balances), затем —
//! если задан адрес user-center — своя запись ключа (урок 29): не заблокирован, право торговли (roleScopes), привязка
//! к IP (bindIps). Не падает: любая проблема — в details с ok = false. Секрет ключа из ответа никуда не пишется.
//! Кто вызывает: mod.rs (check_connection).
//! От чего зависит: account.rs, rest.rs (signed_secret), ctx.rs.
//! Настройки: venue.user_center_url [xt] (необязательная, официально https://api.xt.com), venue.api_ip_bound,
//! venue.api_ip [xt].
//!
//! Официально:
//! - GET /v4/user/account/api-key (https://doc.xt.com/#user_center_apikeyqueryUserApiKey, адрес api.xt.com —
//!   `_user_center_documentation/restApi.md`): [{accessKey, secretKey, bindIps, isLock («0 not locked, 1 locked»),
//!   roleScopes («QUERY_TRADE: trade is enabled; QUERY_NO_TRADE: trade is not enabled»), userAccountLevel}]. Раздел
//!   описан для основного и субаккаунтов — доступен ли обычному ключу, не ясно (ошибка — пояснение, не «не подключено»).
//! - Права при создании ключа: QUERY, TRADE («spot and margin trade»), WITHDRAW, LEVER_LOAN, FUTURE, TRANSFER
//!   (createAccountApiKey.md); в ответе запроса видны только QUERY_TRADE / QUERY_NO_TRADE — право WITHDRAW так не
//!   проверить: проверяется первым выводом (AUTH_106 «Exceeded apikey permission»).

use serde_json::Value;

use crate::gateway::contract::ConnectionCheck;

use super::account;
use super::ctx::Ctx;
use super::rest;

/// Путь запроса своих ключей (user-center).
const API_KEYS: &str = "/v4/user/account/api-key";

/// Проверка подключения. Ошибок нет: всё — в ConnectionCheck.
pub async fn check(ctx: &Ctx) -> ConnectionCheck {
    let key = match ctx.keys() {
        Ok((k, _)) => k,
        Err(e) => return fail(e.message),
    };
    if let Err(e) = account::raw(ctx).await {
        return fail(format!(
            "Ключ XT не принят на GET /v4/balances: {}",
            e.message
        ));
    }
    let Ok(base) = ctx.text("venue.user_center_url") else {
        return ok("ключ и подпись приняты (баланс спота читается); права ключа не проверены — venue.user_center_url не задан; право вывода проверится первым выводом".into());
    };
    let list = match rest::signed_secret(ctx, base, API_KEYS, vec![]).await {
        Ok(v) => v,
        Err(e) => {
            return ok(format!(
                "ключ и подпись приняты; запись ключа в user-center не прочитана ({}) — права проверятся ордером и выводом",
                e.message
            ));
        }
    };
    let rows = list.as_array().cloned().unwrap_or_default();
    let Some(me) = rows
        .iter()
        .find(|r| r.get("accessKey").and_then(Value::as_str) == Some(key.as_str()))
    else {
        return fail(format!(
            "В списке ключей user-center XT ({} шт.) нет текущего ключа",
            rows.len()
        ));
    };
    let text = |f: &str| {
        me.get(f)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    if me.get("isLock").and_then(Value::as_i64) == Some(1) {
        return fail("Ключ XT заблокирован (isLock = 1)".into());
    }
    let scopes = text("roleScopes");
    if scopes == "QUERY_NO_TRADE" {
        return fail("У ключа XT нет права торговли (roleScopes = QUERY_NO_TRADE)".into());
    }
    let ip = match ip_note(ctx, &text("bindIps")) {
        Ok(n) => n,
        Err(why) => return fail(why),
    };
    ok(format!(
        "ключ не заблокирован, roleScopes {scopes}, {ip}; право вывода (WITHDRAW) в ответе не видно — проверится первым выводом"
    ))
}

/// Привязка к IP по настройкам и bindIps ключа. Возвращает: пояснение или текст причины отказа.
fn ip_note(ctx: &Ctx, bind: &str) -> Result<String, String> {
    let ips: Vec<&str> = bind
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let bound = ctx.flag("venue.api_ip_bound").map_err(|e| e.message)?;
    if !bound {
        return Ok(format!(
            "привязка к IP не требуется (у ключа: {})",
            show(&ips)
        ));
    }
    let ip = ctx.text("venue.api_ip").map_err(|e| e.message)?;
    if !ips.contains(&ip.as_str()) {
        return Err(format!(
            "Привязка к IP включена, но IP {ip} нет в bindIps ключа XT ({})",
            show(&ips)
        ));
    }
    Ok(format!("ключ привязан к IP {ip}"))
}

/// Список IP текстом. Ошибок нет.
fn show(ips: &[&str]) -> String {
    if ips.is_empty() {
        "не привязан".into()
    } else {
        ips.join(", ")
    }
}

/// «Подключено». Ошибок нет.
fn ok(details: String) -> ConnectionCheck {
    ConnectionCheck {
        ok: true,
        details: format!("Подключено: {details}"),
    }
}

/// Результат «не подключено». Ошибок нет.
fn fail(details: String) -> ConnectionCheck {
    ConnectionCheck { ok: false, details }
}
