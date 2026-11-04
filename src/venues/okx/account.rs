//! Файл: venues/okx/account.rs
//! Что делает: балансы торгового счёта OKX (Trading, 18) — все и одной монеты (решение 113), и кнопка «Проверить
//! подключение»: ключ принят, права своей записи ключа (урок 29), режим счёта Spot, основной счёт (не субсчёт — вывод
//! только с основного), монета комиссии feeType, привязка к IP.
//! Кто вызывает: mod.rs (функции контракта).
//! От чего зависит: rest.rs, num.rs, ctx.rs.
//! Настройки: venue.api_ip_bound, venue.api_ip [okx].
//!
//! Официально (https://www.okx.com/docs-v5/en/, «Trading Account → REST API»):
//! - «Get balance» GET /api/v5/account/balance (10 запросов / 2 с, User ID): ccy — «Single currency or multiple
//!   currencies (no more than 20) separated with comma»; details[] — ccy, cashBal «Cash balance», availBal «Available
//!   balance of currency», frozenBal «Frozen balance of currency», eq.
//! - «Get account configuration» GET /api/v5/account/config (5 запросов / 2 с): perm «read_only : Read trade : Trade
//!   withdraw : Withdraw» (права ключа запроса); acctLv — 1 Spot, 2 Futures, 3 Multi-currency margin, 4 Portfolio margin;
//!   feeType «0: fee is charged in the currency you receive from the trade», 1 — «always charged in the quote currency»;
//!   uid / mainUid — «The request comes from the main account if uid equals mainUid»; ip — «IP addresses linked to the API
//!   key, comma-separated. It is an empty string if none are bound». Вывод: «Common sub-account does not support
//!   withdrawal». Перевод Funding ↔ Trading — «Only API keys with Transfer privilege»: отдельного значения perm для него
//!   в документации нет (право «Trade» описано как «place and cancel orders, funding transfer…» — проверить на живом
//!   переводе).

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::{num, rest};

/// Права, без которых бот не работает (значения perm).
const REQUIRED: [&str; 2] = ["trade", "withdraw"];
/// Режим счёта Spot — acctLv «1» (бот только спот: CLAUDE.md).
const SPOT_MODE: &str = "1";
/// Комиссия в получаемой монете — feeType «0» (как считает ядро: покупка уменьшает купленное количество).
const FEE_IN_RECEIVED: &str = "0";

/// Балансы торгового счёта; ccy — одна монета (None — все). Ошибки: ключи, связь, ответ OKX.
pub async fn balances(ctx: &Ctx, ccy: Option<&str>) -> GatewayResult<Vec<Balance>> {
    let q = ccy
        .map(|c| vec![("ccy", c.to_string())])
        .unwrap_or_default();
    let data = rest::get(ctx, "/api/v5/account/balance", q).await?;
    parse_balances(ctx, &data)
}

/// Разбор Get balance: строки details первой записи (free = availBal, locked = frozenBal). Пустой data — пустой счёт
/// (урок 27). Ошибка BadResponse — нет полей.
pub fn parse_balances(ctx: &Ctx, data: &serde_json::Value) -> GatewayResult<Vec<Balance>> {
    let Some(row) = data.as_array().and_then(|a| a.first()) else {
        return Ok(vec![]);
    };
    let Some(details) = row.get("details").and_then(|d| d.as_array()) else {
        return Ok(vec![]);
    };
    details
        .iter()
        .map(|b| {
            Ok(Balance {
                venue: ctx.venue(),
                asset: num::text(b, "ccy")?,
                free: num::dec_opt(b, "availBal")?.unwrap_or_default(),
                locked: num::dec_opt(b, "frozenBal")?.unwrap_or_default(),
            })
        })
        .collect()
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
    let data = rest::get(ctx, "/api/v5/account/config", vec![])
        .await
        .map_err(|e| e.message)?;
    let c = rest::first(&data, "account/config").map_err(|e| e.message)?;
    let f = |k: &str| num::text_opt(c, k).unwrap_or_default();
    let perms = split(&f("perm"));
    let missing: Vec<&str> = REQUIRED
        .into_iter()
        .filter(|p| !perms.iter().any(|x| x == p))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "У ключа OKX нет прав: {} (есть: {}). trade — «Торговля», withdraw — «Вывод»",
            missing.join(", "),
            perms.join(", ")
        ));
    }
    if f("acctLv") != SPOT_MODE {
        return Err(format!(
            "Режим счёта OKX acctLv = {} — нужен Spot mode (1): бот только спот, ордера tdMode = cash",
            f("acctLv")
        ));
    }
    if f("uid") != f("mainUid") {
        return Err(
            "Ключ OKX — субсчёта (uid ≠ mainUid): «Common sub-account does not support withdrawal»"
                .into(),
        );
    }
    if f("feeType") != FEE_IN_RECEIVED {
        return Err(format!(
            "OKX feeType = {} — нужен 0 (комиссия в получаемой монете, как считает бот); сменить: Set fee type",
            f("feeType")
        ));
    }
    let ips = split(&f("ip"));
    let ip_note = if ctx.flag("venue.api_ip_bound").map_err(|e| e.message)? {
        let ip = ctx.text("venue.api_ip").map_err(|e| e.message)?;
        if !ips.contains(&ip) {
            return Err(format!(
                "Привязка к IP включена, но IP {ip} нет у ключа OKX (там: {})",
                list(&ips)
            ));
        }
        format!("ключ привязан к IP {ip}")
    } else {
        format!(
            "привязка к IP не требуется (на OKX: {}; ключ без IP с правами trade/withdraw OKX отключает после 14 дней простоя)",
            list(&ips)
        )
    };
    Ok(format!(
        "Подключено. Режим счёта: Spot (acctLv = 1), основной счёт (uid = mainUid), комиссия в получаемой монете \
         (feeType = 0). Права: {}; {ip_note}; метка ключа «{}»",
        perms.join(", "),
        f("label")
    ))
}

/// Строка списка → элементы (разделители: запятая, пробелы). Ошибок нет.
fn split(s: &str) -> Vec<String> {
    s.split([',', ' '])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

/// Список IP для текста.
fn list(ips: &[String]) -> String {
    if ips.is_empty() {
        "без привязки".into()
    } else {
        ips.join(", ")
    }
}
