//! Файл: venues/binance/delist.rs
//! Что делает: делистинги Binance (решение 175, контракт delistings) — график делистинга пар спота: время и символы;
//! символ → тикер монеты по справочнику пар (symbols.rs, только пары с venue.quote_asset).
//! Кто вызывает: mod.rs (Venue::delistings), tests/delist_parse.rs.
//! От чего зависит: rest.rs, symbols.rs, errors.rs, ctx.rs, core/types (Delistings).
//! Настройки: venue.quote_asset, venue.symbols_refresh_s (через symbols.rs), venue.recv_window_ms (подпись).
//!
//! Официальная документация: Wallet → «Get symbols delist schedule for spot (MARKET_DATA)» —
//! GET /sapi/v1/spot/delist-schedule, «Weight(IP): 100»
//! https://developers.binance.com/docs/wallet/others/delist-schedule ; спецификация OpenAPI
//! https://github.com/binance/binance-api-swagger (spot_api.yaml): параметры recvWindow, timestamp, signature (запрос
//! подписывается), ответ — массив {delistTime (int64, мс; пример 1686161202000), symbol (массив строк; пример
//! ["ADAUSDT","BNBUSDT"])}, оба поля обязательны. Страница документации на сайте в этой сессии не прочитана (сайт
//! переделан) — разбор строго по спецификации; живой ответ записать и сверить (tests/README.md). Другой ключ в живом
//! ответе — ошибка разбора в журнале core.delist, сделки не блокируются. Времени закрытия вывода в ответе нет.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use reqwest::Method;
use serde_json::Value;

use crate::core::types::{DelistItem, Delistings};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;

/// Путь эндпоинта и источник для человека.
const PATH: &str = "/sapi/v1/spot/delist-schedule";
const SOURCE: &str = "Binance GET /sapi/v1/spot/delist-schedule: delistTime";

/// Что делает: запрашивает график делистинга и сопоставляет символы с монетами справочника пар. Возвращает:
/// Delistings по фактам API (символы других котируемых монет пропускаются). Ошибки: ключи, связь, ответ Binance,
/// справочник пар не получен, неверный формат.
pub async fn delistings(ctx: &Ctx, symbols: &Symbols) -> GatewayResult<Delistings> {
    symbols.ensure_fresh(ctx).await?;
    let data = super::rest::signed(ctx, Method::GET, PATH, vec![], Op::Other).await?;
    let raw = parse(&data)?;
    let items = raw
        .into_iter()
        .filter_map(|(symbol, at)| {
            symbols.ticker_of(&symbol).map(|ticker| DelistItem {
                ticker,
                symbol,
                trading_end: Some(at),
                withdraw_end: None,
                source: SOURCE.into(),
            })
        })
        .collect();
    Ok(Delistings::of(items))
}

/// Что делает: разбор ответа. Принимает: ответ. Возвращает: символ → время делистинга (у символа в нескольких записях —
/// самое раннее). Ошибка BadResponse — не массив, нет delistTime или списка символов.
pub fn parse(data: &Value) -> GatewayResult<HashMap<String, DateTime<Utc>>> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("delist-schedule не массив", &data.to_string()))?;
    let mut out: HashMap<String, DateTime<Utc>> = HashMap::new();
    for e in list {
        let ms = e
            .get("delistTime")
            .and_then(Value::as_i64)
            .ok_or_else(|| super::errors::bad("нет delistTime", &e.to_string()))?;
        let at = DateTime::<Utc>::from_timestamp_millis(ms)
            .ok_or_else(|| super::errors::bad("delistTime вне диапазона", &e.to_string()))?;
        let syms = e
            .get("symbol")
            .and_then(Value::as_array)
            .ok_or_else(|| super::errors::bad("нет списка символов", &e.to_string()))?;
        for s in syms.iter().filter_map(Value::as_str) {
            let slot = out.entry(s.to_string()).or_insert(at);
            if at < *slot {
                *slot = at;
            }
        }
    }
    Ok(out)
}
