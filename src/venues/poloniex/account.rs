//! Файл: venues/poloniex/account.rs
//! Что делает: балансы спот-счёта Poloniex (все; одной монеты — фильтром: отдельного запроса нет) и кнопка «Проверить
//! подключение»: ключ и подпись приняты, счёт не заблокирован, ставки комиссии и скидка TRX читаются; права торговли и
//! вывода через API не проверяемы (урок 29) — сказано в тексте.
//! Кто вызывает: mod.rs (функции контракта), all_balances.rs (Спот), user_stream.rs (опорные остатки).
//! От чего зависит: rest.rs, num.rs, fees.rs, ctx.rs.
//! Настройки: нет своих.
//!
//! Официально (https://api-docs.poloniex.com/spot/api/private/account):
//! - All Account Balances GET /accounts/balances?accountType=SPOT — [{accountId, accountType, balances: [{currencyId,
//!   currency, available («Can be negative when margin is used»), hold}]}]. Баланса одной монеты отдельным запросом нет.
//! - Account Information GET /accounts — accountState («NORMAL», «LOCKED»).
//! - Права ключа: эндпоинта со списком прав ключа в Spot API нет; сказано только «withdrawal privilege must be enabled
//!   for your API key» (Withdraw Currency). Белого списка IP и адресов вывода в API нет — настраиваются в кабинете
//!   (https://poloniex.com/profile/settings/spot-api-key).

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{fees, num, rest};

/// Балансы спот-счёта. Ошибки: ключи, связь, ответ Poloniex.
pub async fn balances(ctx: &Ctx) -> GatewayResult<Vec<Balance>> {
    let q = vec![("accountType", "SPOT".to_string())];
    let data = rest::signed(ctx, Method::GET, "/accounts/balances", q, None, Op::Other).await?;
    spot_rows(ctx, &data)
}

/// Строки спот-счёта из ответа All Account Balances: free = available, locked = hold (счета accountType SPOT).
/// Ошибки: BadResponse — ответ не массив, нет полей.
pub fn spot_rows(ctx: &Ctx, data: &Value) -> GatewayResult<Vec<Balance>> {
    let list = data.as_array().ok_or_else(|| {
        super::errors::bad("accounts/balances: ответ не массив", &data.to_string())
    })?;
    let mut out = Vec::new();
    for acc in list {
        if num::text_opt(acc, "accountType").is_some_and(|t| t != "SPOT") {
            continue;
        }
        for b in num::arr(acc, "balances")? {
            out.push(Balance {
                venue: ctx.venue(),
                asset: num::text(b, "currency")?.to_ascii_uppercase(),
                free: num::dec(b, "available")?,
                locked: num::dec_opt(b, "hold")?.unwrap_or(Decimal::ZERO),
            });
        }
    }
    Ok(out)
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
    let accounts = rest::signed(ctx, Method::GET, "/accounts", vec![], None, Op::Other)
        .await
        .map_err(|e| e.message)?;
    let states: Vec<String> = accounts
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| {
                    format!(
                        "{} {}",
                        num::text_opt(x, "accountType").unwrap_or_default(),
                        num::text_opt(x, "accountState").unwrap_or_default()
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(bad) = states.iter().find(|s| !s.ends_with(" NORMAL")) {
        return Err(format!(
            "Счёт Poloniex не в состоянии NORMAL: {bad} (все: {})",
            states.join(", ")
        ));
    }
    let fee = match fees::fetch(ctx).await {
        Ok(i) if i.trx_discount => format!(
            "ставка taker {}, скидка TRX включена — расчёт по обычной ставке (худший случай)",
            i.general.taker.normalize()
        ),
        Ok(i) => format!(
            "ставка taker {}, скидка TRX выключена",
            i.general.taker.normalize()
        ),
        Err(e) => {
            return Err(format!(
                "Ставки комиссии Poloniex не прочитаны: {}",
                e.message
            ));
        }
    };
    Ok(format!(
        "Подключено. Счёт: {}; {fee}. Права торговли и вывода через API Poloniex не проверяются (эндпоинта прав ключа \
         нет) — у ключа должны быть включены торговля и вывод, без фьючерсов (у фьючерсов отдельные ключи). Белый \
         список IP и адресов вывода — в кабинете Poloniex",
        states.join(", ")
    ))
}
