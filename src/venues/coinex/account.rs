//! Файл: venues/coinex/account.rs
//! Что делает: балансы спот-счёта CoinEx и кнопка «Проверить подключение»: ключ и подпись приняты (GET /account/info),
//! IP не заблокирован (код 4007), баланс спота читается, ставка комиссии пары читается и настройка её единиц задана.
//! Права ключа основного аккаунта (торговля, вывод, отсутствие фьючерсов) API CoinEx не отдаёт — это сказано в тексте
//! «Подключено» (проверить в кабинете), а не выдумывается (уроки 19, 29).
//! Кто вызывает: mod.rs (функции контракта), all_balances.rs (Спот).
//! От чего зависит: rest.rs, num.rs, fees.rs, ctx.rs.
//! Настройки: venue.fee_rate_percent [coinex] (только показ).
//!
//! Официально:
//! - Get Balance in Spot Account GET /assets/spot/balance (без параметров) https://docs.coinex.com/api/v2/assets/balance/http/get-spot-balance
//!   — data[] {ccy, available, frozen «funds locked in open orders»}. Запроса одной монеты нет — фильтр свой.
//! - Get Account Info GET /account/info https://docs.coinex.com/api/v2/account/info/http/get-account-info — {user_id}.
//! - Права ключа (trade_enabled, withdraw_enabled, ip_whitelist, expires_at) есть только у ключей СУБАККАУНТОВ
//!   (GET /account/subs/api-detail); для ключа основного аккаунта запроса прав нет (вопрос Тиграну, анализ п. 10).
//!   Ошибки ключа: 4005 (новый ключ — 10 минут до активации, FAQ), 4006, 4007 (IP), 4010/4017 (время), 4011/4115.

use reqwest::Method;
use serde_json::Value;

use crate::core::types::Balance;
use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::{fees, num, rest};

/// Балансы спот-счёта (все). Ошибки: ключи, связь, ответ CoinEx.
pub async fn balances(ctx: &Ctx) -> GatewayResult<Vec<Balance>> {
    let path = "/v2/assets/spot/balance";
    let data = rest::signed(ctx, Method::GET, path, vec![], None, Op::Other).await?;
    spot_rows(ctx, &data)
}

/// Строки спот-счёта: free = available, locked = frozen. Ошибки: BadResponse — data не массив, нет полей.
pub fn spot_rows(ctx: &Ctx, data: &Value) -> GatewayResult<Vec<Balance>> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("spot/balance: data не массив", &data.to_string()))?;
    list.iter()
        .map(|b| {
            Ok(Balance {
                venue: ctx.venue(),
                asset: num::text(b, "ccy")?.to_ascii_uppercase(),
                free: num::dec(b, "available")?,
                locked: num::dec_opt(b, "frozen")?.unwrap_or_default(),
            })
        })
        .collect()
}

/// Проверка подключения. Принимает: рынок для пробы ставки комиссии (любая пара справочника; None — не пробуем).
/// Не падает: любая проблема — в details с ok=false.
pub async fn check(ctx: &Ctx, market: Option<String>) -> ConnectionCheck {
    match run(ctx, market).await {
        Ok(details) => ConnectionCheck { ok: true, details },
        Err(details) => ConnectionCheck { ok: false, details },
    }
}

/// Сама проверка. Возвращает: текст «Подключено…». Ошибка — точная причина.
async fn run(ctx: &Ctx, market: Option<String>) -> Result<String, String> {
    let get = |path: &'static str| async move {
        rest::signed(ctx, Method::GET, path, vec![], None, Op::Other)
            .await
            .map_err(|e| e.message)
    };
    let info = get("/v2/account/info").await?;
    let uid = num::text_opt(&info, "user_id").unwrap_or_else(|| "—".into());
    let spot = balances(ctx)
        .await
        .map_err(|e| format!("баланс спота: {}", e.message))?;
    // Ставка — как пришла (без деления): по ней Тигран решает venue.fee_rate_percent (единицы не описаны, fees.rs).
    let unit = match ctx.flag("venue.fee_rate_percent") {
        Ok(true) => "в процентах",
        Ok(false) => "доля",
        Err(_) => "не задана — ставок комиссии и котировок CoinEx нет",
    };
    let fee = match market {
        None => format!(
            "справочник пар не получен — ставка не проверена; venue.fee_rate_percent: {unit}"
        ),
        Some(m) => match fees::fetch(ctx, &m, false).await {
            Ok(r) => format!(
                "taker_rate {m} как пришёл = {}; venue.fee_rate_percent: {unit}",
                r.taker.normalize()
            ),
            Err(e) => format!("ставка {m} не прочитана: {}", e.message),
        },
    };
    Ok(format!(
        "Подключено (user_id {uid}). Спот читается ({} монет), {fee}. Права ключа (торговля, вывод; без фьючерсов и \
         маржи) и белый список IP API CoinEx для ключа основного аккаунта не отдаёт — проверьте в кабинете; адреса \
         вывода — в белый список ключа (без вторичной проверки, FAQ 3310)",
        spot.len()
    ))
}
