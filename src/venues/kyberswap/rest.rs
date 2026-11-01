//! Файл: venues/kyberswap/rest.rs
//! Что делает: клиент REST агрегатора KyberSwap (EVM-свопы) — две операции по официальной документации: GET
//! /{chain}/api/v1/routes (котировка-маршрут) и POST /{chain}/api/v1/route/build (сборка транзакции из маршрута).
//! Разбор ответа {code, message, data}, перевод ошибок в коды шлюза, запись сырого ответа в журнал.
//! Кто вызывает: quote.rs, trade.rs, market.rs, connection.rs.
//! От чего зависит: ctx.rs, errors.rs, gateway/http.rs, alloy-primitives, reqwest, serde_json.
//! Настройки: kyber.base_url, kyber.chain, kyber.client_id, kyber.http_timeout_ms, http.*.
//!
//! Эндпоинты, поля и заголовок X-Client-Id — строго по официальной спецификации:
//! https://docs.kyberswap.com/kyberswap-solutions/kyberswap-aggregator/aggregator-api-specification/evm-swaps
//! GET routes: query tokenIn, tokenOut, amountIn (в wei); ответ data.routeSummary {amountOut, gas, gasPrice, gasUsd,
//! route, routeID, checksum, timestamp} и data.routerAddress. POST route/build: тело {routeSummary, sender, recipient,
//! slippageTolerance (bps 0–2000), deadline (Unix-сек)}; ответ data {data (calldata), routerAddress, transactionValue,
//! amountIn, amountOut, gas}.

use std::str::FromStr;

use alloy_primitives::{Address, Bytes, U256};
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::http;

use super::ctx::{Ctx, ID};
use super::errors;

/// Источник для текста ошибок и сырого журнала.
const SOURCE: &str = "KyberSwap агрегатор";

/// Маршрут-котировка из GET /routes: разобранные числа и сырой routeSummary для передачи в build.
#[derive(Debug, Clone)]
pub struct Route {
    /// Сколько монеты выхода получим (в минимальных единицах токена выхода).
    pub amount_out: U256,
    /// Оценка газа свопа (единиц газа).
    pub gas_units: u64,
    /// Цена газа (wei) по оценке агрегатора; None — не прислал (тогда стоимость газа берётся из gas_usd).
    pub gas_price: Option<U256>,
    /// Стоимость газа в USD по оценке агрегатора; None — не прислал.
    pub gas_usd: Option<Decimal>,
    /// Адрес роутера (из ответа, не зашит).
    pub router: Address,
    /// Сырой routeSummary — передаётся в POST build ровно как есть (документация: «ровно как из GET routes»).
    pub summary: Value,
}

/// Собранная транзакция из POST /route/build.
#[derive(Debug, Clone)]
pub struct Built {
    /// Готовый calldata для роутера.
    pub data: Bytes,
    /// Адрес роутера (куда слать транзакцию).
    pub router: Address,
    /// Сумма нативной монеты транзакции (для свопов с BNB на входе), wei.
    pub value: U256,
    /// Вход и выход после сборки (минимальные единицы).
    pub amount_in: U256,
    pub amount_out: U256,
}

/// GET /routes: котировка-маршрут USDT↔монета. Принимает: токен входа, токен выхода, вход (wei).
/// Ошибки: NoRoute (маршрута нет), сеть, лимиты, BadResponse.
pub async fn routes(
    ctx: &Ctx,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
) -> GatewayResult<Route> {
    let query =
        format!("tokenIn={token_in}&tokenOut={token_out}&amountIn={amount_in}&gasInclude=true");
    let data = send(ctx, Method::GET, "/routes", &query, None).await?;
    let summary = data
        .get("routeSummary")
        .cloned()
        .ok_or_else(|| errors::bad("нет data.routeSummary", SOURCE, &data.to_string()))?;
    let amount_out = u256_field(&summary, "amountOut")?;
    if amount_out.is_zero() {
        return Err(ctx.err(ErrorCode::NoRoute, "агрегатор вернул amountOut = 0"));
    }
    let gas_units = summary
        .get("gas")
        .and_then(as_u64)
        .ok_or_else(|| errors::bad("нет routeSummary.gas", SOURCE, &summary.to_string()))?;
    let gas_price = summary.get("gasPrice").and_then(u256_any);
    let gas_usd = summary.get("gasUsd").and_then(as_decimal);
    let router = addr_field(&data, "routerAddress")?;
    Ok(Route {
        amount_out,
        gas_units,
        gas_price,
        gas_usd,
        router,
        summary,
    })
}

/// POST /route/build: собирает транзакцию из маршрута. Принимает: маршрут, отправителя, получателя, допуск bps, срок.
/// Ошибки: сеть, BadResponse, ответ-ошибка агрегатора.
pub async fn build(
    ctx: &Ctx,
    route: &Route,
    sender: Address,
    recipient: Address,
    slippage_bps: u64,
    deadline: u64,
) -> GatewayResult<Built> {
    let body = json!({
        "routeSummary": route.summary,
        "sender": sender,
        "recipient": recipient,
        "slippageTolerance": slippage_bps,
        "deadline": deadline,
    });
    let data = send(ctx, Method::POST, "/route/build", "", Some(body)).await?;
    let calldata: Bytes = data
        .get("data")
        .and_then(Value::as_str)
        .and_then(|s| Bytes::from_str(s).ok())
        .ok_or_else(|| errors::bad("нет data.data (calldata)", SOURCE, &data.to_string()))?;
    let router = addr_field(&data, "routerAddress")?;
    let value = data
        .get("transactionValue")
        .and_then(u256_any)
        .unwrap_or(U256::ZERO);
    let amount_in = u256_field(&data, "amountIn")?;
    let amount_out = u256_field(&data, "amountOut")?;
    Ok(Built {
        data: calldata,
        router,
        value,
        amount_in,
        amount_out,
    })
}

/// Отправляет запрос и возвращает поле data ответа {code, message, data}.
/// Принимает: метод, путь (после /{chain}/api/v1), строку запроса, тело (для POST). Ошибки: сеть, HTTP, ответ-ошибка.
async fn send(
    ctx: &Ctx,
    method: Method,
    path: &str,
    query: &str,
    body: Option<Value>,
) -> GatewayResult<Value> {
    let base = ctx.api_base()?;
    let lim = ctx.limits()?;
    let url = if query.is_empty() {
        format!("{base}{path}")
    } else {
        format!("{base}{path}?{query}")
    };
    let mut req = ctx.http.request(method, url);
    if let Ok(client_id) = ctx.text("kyber.client_id")
        && !client_id.is_empty()
    {
        req = req.header("X-Client-Id", client_id);
    }
    if let Some(b) = &body {
        req = req.json(b);
    }
    let (status, text) = http::exchange(req, lim)
        .await
        .map_err(|f| errors::from_fail(&f, &lim, SOURCE))?;
    ctx.raw.write(ID, &format!("rest {path}"), &text);
    let v: Value = serde_json::from_str(&text).map_err(|_| {
        if status.is_success() {
            errors::bad("ответ не JSON", SOURCE, &text)
        } else {
            errors::from_http(status.as_u16(), SOURCE, &text)
        }
    })?;
    match v.get("code").and_then(Value::as_i64) {
        Some(0) => v
            .get("data")
            .cloned()
            .ok_or_else(|| errors::bad("code 0, но нет поля data", SOURCE, &text)),
        Some(code) => {
            let msg = v.get("message").and_then(Value::as_str).unwrap_or_default();
            // Нет маршрута у агрегатора — отдельный код шлюза (ядро трактует как «объём велик», costs_quote.rs).
            let low = msg.to_ascii_lowercase();
            let kind = if low.contains("route") || low.contains("not found") {
                ErrorCode::NoRoute
            } else {
                ErrorCode::BadResponse
            };
            Err(ctx
                .err(kind, format!("{SOURCE}, код {code}: {msg}"))
                .with_raw(&text))
        }
        None if status.is_success() => Ok(v),
        None => Err(errors::from_http(status.as_u16(), SOURCE, &text)),
    }
}

/// U256 из строкового десятичного поля объекта. Ошибки: BadResponse.
fn u256_field(obj: &Value, name: &str) -> GatewayResult<U256> {
    obj.get(name).and_then(u256_any).ok_or_else(|| {
        errors::bad(
            &format!("поле {name} не число U256"),
            SOURCE,
            &obj.to_string(),
        )
    })
}

/// U256 из JSON-значения (строка-десятичное или число). None — не разобрать.
fn u256_any(v: &Value) -> Option<U256> {
    match v {
        Value::String(s) => U256::from_str(s).ok(),
        Value::Number(n) => n.as_u64().map(U256::from),
        _ => None,
    }
}

/// Адрес из строкового поля объекта. Ошибки: BadResponse.
fn addr_field(obj: &Value, name: &str) -> GatewayResult<Address> {
    obj.get(name)
        .and_then(Value::as_str)
        .and_then(|s| Address::from_str(s).ok())
        .ok_or_else(|| errors::bad(&format!("поле {name} не адрес"), SOURCE, &obj.to_string()))
}

/// u64 из строки или числа. None — не разобрать.
fn as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

/// Decimal из строки или числа (например gasUsd). None — не разобрать.
fn as_decimal(v: &Value) -> Option<Decimal> {
    match v {
        Value::String(s) => Decimal::from_str(s).ok(),
        Value::Number(n) => n
            .as_f64()
            .and_then(|f| Decimal::from_str(&f.to_string()).ok()),
        _ => None,
    }
}
