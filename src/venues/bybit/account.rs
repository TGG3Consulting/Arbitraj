//! Файл: venues/bybit/account.rs
//! Что делает: балансы единого торгового счёта Bybit (UNIFIED — там торгует спот), баланс одной монеты и кнопка
//! «Проверить подключение»: ключ действует, есть нужные права и НЕТ прав на деривативы (урок 19), ключ главного
//! аккаунта (вывод и адрес депозита — только у master), аккаунт — единый (UTA), привязка к IP совпадает с настройками.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.api_ip_bound, venue.api_ip [bybit].
//!
//! Официальная документация:
//! - «Get Wallet Balance» GET /v5/account/wallet-balance?accountType=UNIFIED[&coin=] https://bybit-exchange.github.io/docs/v5/account/wallet-balance
//!   — list[0].coin[]: coin, walletBalance, locked («Locked balance due to the Spot open order»), spotBorrow («Old
//!   walletBalance = New walletBalance - spotBorrow»); «By default, currency information with assets or liabilities of
//!   0 is not returned». Свободно = walletBalance − spotBorrow − locked (займов бот не берёт: isLeverage = 0).
//! - «Get API Key Information» GET /v5/user/query-api …/user/apikey-info — readOnly, permissions (Spot: SpotTrade;
//!   Wallet: AccountTransfer, Withdraw; ContractTrade, Options, Derivatives), ips, deadlineDay, uta, isMaster.
//! - «Get Account Info» GET /v5/account/info …/account/account-info — unifiedMarginStatus (1 classic, 3/4 UTA 1.0,
//!   5/6 UTA 2.0 — enum unifiedMarginStatus; «Different Account Modes»: classic «now unavailable»).

use serde_json::Value;

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

const PATH_WALLET: &str = "/v5/account/wallet-balance";
/// Права, без которых бот не работает: (группа, право) — имена из документации.
const REQUIRED: [(&str, &str); 3] = [
    ("Spot", "SpotTrade"),
    ("Wallet", "AccountTransfer"),
    ("Wallet", "Withdraw"),
];
/// Группы прав торговли не спотом (урок 19: фьючерсов и маржи в проекте нет) — должны быть пусты.
const FORBIDDEN: [&str; 3] = ["ContractTrade", "Options", "Derivatives"];

/// Балансы UNIFIED. Ошибки: ключи, связь, ответ Bybit.
pub async fn balances(ctx: &Ctx) -> GatewayResult<Vec<Balance>> {
    wallet(ctx, None).await
}

/// Баланс одной монеты (решение 113: тот же запрос с coin=). None — монеты нет (нулевые Bybit не отдаёт).
pub async fn balance(ctx: &Ctx, asset: &str) -> GatewayResult<Option<Balance>> {
    Ok(wallet(ctx, Some(asset))
        .await?
        .into_iter()
        .find(|b| b.asset == asset))
}

/// Запрос wallet-balance. Ошибки: как balances.
async fn wallet(ctx: &Ctx, coin: Option<&str>) -> GatewayResult<Vec<Balance>> {
    let mut p = vec![("accountType", "UNIFIED".to_string())];
    if let Some(c) = coin {
        p.push(("coin", c.to_string()));
    }
    let data = rest::get(ctx, PATH_WALLET, p, Op::Other).await?;
    let mut out = vec![];
    for acc in num::arr(&data, "list")? {
        for c in acc
            .get("coin")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let total = num::dec(c, "walletBalance")?;
            let borrow = num::dec_opt(c, "spotBorrow")?.unwrap_or_default();
            let locked = num::dec_opt(c, "locked")?.unwrap_or_default();
            out.push(Balance {
                venue: ctx.venue(),
                asset: num::text(c, "coin")?,
                free: total - borrow - locked,
                locked,
            });
        }
    }
    Ok(out)
}

/// Проверка подключения. Не падает: любая проблема — в details с ok=false.
pub async fn check(ctx: &Ctx) -> ConnectionCheck {
    let info = match rest::get(ctx, "/v5/user/query-api", vec![], Op::Other).await {
        Ok(d) => d,
        Err(e) => return fail(e.message),
    };
    if num::int(&info, "readOnly").ok() != Some(0) {
        return fail("Ключ Bybit только на чтение (readOnly ≠ 0) — нужен ключ Read-Write".into());
    }
    if info.get("isMaster").and_then(Value::as_bool) != Some(true) {
        return fail("Ключ не главного аккаунта (isMaster ≠ true): вывод и адрес депозита Bybit — только у master".into());
    }
    if num::int(&info, "uta").ok() != Some(1) {
        return fail("Аккаунт не единый торговый (uta ≠ 1) — V5 спот работает на UTA".into());
    }
    let perms = info.get("permissions").cloned().unwrap_or(Value::Null);
    let has = |g: &str, p: &str| {
        perms
            .get(g)
            .and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(p)))
    };
    let missing: Vec<String> = REQUIRED
        .iter()
        .filter(|(g, p)| !has(g, p))
        .map(|(g, p)| format!("{g}: {p}"))
        .collect();
    if !missing.is_empty() {
        return fail(format!(
            "У ключа нет прав: {} — включите их в Bybit → API Management",
            missing.join(", ")
        ));
    }
    let extra: Vec<&str> = FORBIDDEN
        .iter()
        .copied()
        .filter(|g| {
            perms
                .get(*g)
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
        })
        .collect();
    if !extra.is_empty() {
        return fail(format!(
            "У ключа лишние права торговли не спотом ({}) — снимите их: в проекте только спот",
            extra.join(", ")
        ));
    }
    let mode = match rest::get(ctx, "/v5/account/info", vec![], Op::Other).await {
        Ok(d) => num::int(&d, "unifiedMarginStatus").unwrap_or(-1),
        Err(e) => return fail(e.message),
    };
    if !(3..=6).contains(&mode) {
        return fail(format!(
            "Режим аккаунта unifiedMarginStatus = {mode} — нужен единый (UTA, 3–6)"
        ));
    }
    let ips: Vec<String> = info
        .get("ips")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let ip_note = match ip_note(ctx, &ips) {
        Ok(n) => n,
        Err(m) => return fail(m),
    };
    let term = match num::int(&info, "deadlineDay") {
        Ok(d) => format!("ключу осталось {d} дн."),
        Err(_) => "срок ключа не ограничен".to_string(),
    };
    ConnectionCheck {
        ok: true,
        details: format!(
            "Подключено: права есть (Spot: SpotTrade, Wallet: AccountTransfer, Withdraw), прав деривативов нет, \
             UTA (unifiedMarginStatus {mode}), {ip_note}, {term}"
        ),
    }
}

/// Привязка к IP: включена — IP из venue.api_ip должен быть в ips ключа. Ошибка — текст причины.
fn ip_note(ctx: &Ctx, ips: &[String]) -> Result<String, String> {
    let list = if ips.is_empty() {
        "без привязки".into()
    } else {
        ips.join(", ")
    };
    if !ctx.flag("venue.api_ip_bound").map_err(|e| e.message)? {
        return Ok(format!("привязка к IP не требуется (на Bybit: {list})"));
    }
    let ip = ctx.text("venue.api_ip").map_err(|e| e.message)?;
    if !ips.contains(&ip) {
        return Err(format!(
            "Привязка к IP включена, но IP {ip} нет в списке ключа на Bybit (там: {list})"
        ));
    }
    Ok(format!("ключ привязан к IP {ip}"))
}

/// Результат «не подключено».
fn fail(details: String) -> ConnectionCheck {
    ConnectionCheck { ok: false, details }
}
