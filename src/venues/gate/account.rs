//! Файл: venues/gate/account.rs
//! Что делает: балансы спот-счёта Gate (все и одной монеты — решение 113) и кнопка «Проверить подключение»: ключ
//! принят, режим счёта классический, права спота на запись и кошелька есть, IP; права записи фьючерсов и маржи —
//! только предупреждение в тексте «Подключено» (решение 243).
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs, key_info.rs (своя запись в списке /account/main_keys).
//! Настройки: venue.api_ip_bound, venue.api_ip [gate].
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/ (SDK https://github.com/gate/gateapi-python):
//! - GET /spot/accounts?currency= («List spot trading accounts», SpotAccount): currency, available, locked.
//! - GET /account/detail (AccountDetail): ip_whitelist, currency_pairs, key.mode («1 - Classic mode, 2 - Legacy unified
//!   mode»). Описание создания ордера: у единого счёта ордер идёт с единого счёта — бот этого режима не поддерживает
//!   (депозит «Funds Credited to Spot Account», балансы — /spot/accounts): режим 2 — «не подключено» (вопрос Тиграну).
//! - GET /account/main_keys (AccountKeyInfo): state («1 - Normal, 2 - Locked, 3 - Frozen»), perms[] — name (wallet, spot,
//!   futures, delivery, margin, unified, options, loan, copy …), read_only. Права (раздел «API key permissions»):
//!   спот, кошелёк (переводы и история), вывод (POST /withdrawals); GET — чтение, остальное — запись. Права «вывод»
//!   в перечне имён perms нет — проверяется только первым выводом (вопрос Тиграну, docs/TASKS.md).

use reqwest::Method;
use serde_json::Value;

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Права записи, которые боту не нужны (урок 19: бот — только спот и ввод/вывод). Есть — предупреждение, не сбой
/// (решение 243).
const FORBIDDEN_WRITE: [&str; 7] = [
    "futures", "delivery", "margin", "unified", "options", "loan", "copy",
];

/// Балансы спот-счёта. Принимает: монету (None — все). Ошибки: ключи, связь, ответ Gate.
pub async fn balances(ctx: &Ctx, currency: Option<&str>) -> GatewayResult<Vec<Balance>> {
    let p = currency
        .map(|c| vec![("currency", c.to_string())])
        .unwrap_or_default();
    let data = rest::signed(ctx, Method::GET, "/spot/accounts", p, None, Op::Other).await?;
    let arr = data
        .as_array()
        .ok_or_else(|| super::errors::bad("/spot/accounts не массив", &data.to_string()))?;
    arr.iter()
        .map(|b| {
            Ok(Balance {
                venue: ctx.venue(),
                asset: num::text(b, "currency")?,
                free: num::dec(b, "available")?,
                locked: num::dec(b, "locked")?,
            })
        })
        .collect()
}

/// Проверка подключения. Не падает: любая проблема — в details с ok=false.
pub async fn check(ctx: &Ctx) -> ConnectionCheck {
    let detail =
        match rest::signed(ctx, Method::GET, "/account/detail", vec![], None, Op::Other).await {
            Ok(d) => d,
            Err(e) => return fail(e.message),
        };
    let mode = detail.pointer("/key/mode").and_then(Value::as_i64);
    if mode != Some(1) {
        return fail(format!(
            "Режим ключа Gate key.mode = {mode:?}: бот работает только с классическим режимом (1) — единый счёт не поддержан"
        ));
    }
    let keys = match rest::signed(
        ctx,
        Method::GET,
        "/account/main_keys",
        vec![],
        None,
        Op::Other,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            return fail(format!(
                "Права ключа не прочитаны (/account/main_keys): {}",
                e.message
            ));
        }
    };
    // Ответ — список всех ключей аккаунта: берём запись своего ключа (key_info.rs).
    let my_key = match ctx.keys() {
        Ok((k, _)) => k,
        Err(e) => return fail(e.message),
    };
    let Some(keys) = super::key_info::own(&keys, &my_key) else {
        return fail(
            "Ключа бота нет в списке ключей аккаунта Gate (/account/main_keys) — проверьте gate.api_key".into(),
        );
    };
    if let Some(s) = keys
        .get("state")
        .and_then(Value::as_i64)
        .filter(|s| *s != 1)
    {
        return fail(format!(
            "Ключ Gate не действует: state = {s} (2 — заблокирован, 3 — заморожен)"
        ));
    }
    let perms: Vec<(String, bool)> = keys
        .get("perms")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    Some((
                        num::text_opt(p, "name")?,
                        p.get("read_only") == Some(&Value::Bool(true)),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let has = |n: &str, write: bool| perms.iter().any(|(x, ro)| x == n && (!write || !ro));
    if !has("spot", true) {
        return fail("У ключа нет права спота на запись (perms: spot, read_only = false) — включите в Gate → API Keys".into());
    }
    if !has("wallet", false) {
        return fail(
            "У ключа нет права кошелька (perms: wallet) — история выводов и депозитов не читается"
                .into(),
        );
    }
    let extra: Vec<&str> = FORBIDDEN_WRITE
        .iter()
        .copied()
        .filter(|n| has(n, true))
        .collect();
    // Лишние права — только предупреждение, не сбой (решение 243): бот их не использует.
    let extra = if extra.is_empty() {
        String::new()
    } else {
        format!("; у ключа есть лишние права записи: {}", extra.join(", "))
    };
    let ips = strings(detail.get("ip_whitelist"));
    let ip_note = match ip(ctx, &ips) {
        Ok(n) => n,
        Err(m) => return fail(m),
    };
    let pairs = strings(detail.get("currency_pairs"));
    let pairs_note = if pairs.is_empty() {
        "белого списка пар нет".to_string()
    } else {
        format!("белый список пар: {}", pairs.len())
    };
    ConnectionCheck {
        ok: true,
        details: format!(
            "Подключено: классический режим, права spot и wallet есть, {ip_note}, {pairs_note}; право вывода проверяется первым выводом{extra}"
        ),
    }
}

/// Привязка к IP по настройкам. Возвращает: пояснение или текст причины отказа.
fn ip(ctx: &Ctx, ips: &[String]) -> Result<String, String> {
    let bound = ctx.flag("venue.api_ip_bound").map_err(|e| e.message)?;
    if !bound {
        return Ok(format!(
            "привязка к IP не требуется (на Gate: {})",
            list(ips)
        ));
    }
    let ip = ctx.text("venue.api_ip").map_err(|e| e.message)?;
    if !ips.contains(&ip) {
        return Err(format!(
            "Привязка к IP включена, но IP {ip} нет в белом списке ключа на Gate (там: {})",
            list(ips)
        ));
    }
    Ok(format!("ключ привязан к IP {ip}"))
}

/// Массив строк из поля. Нет поля или не массив — пусто. Ошибок нет.
fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
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
