//! Файл: venues/kucoin/tap_decode.rs
//! Что делает: разбор записанного кадра окна KuCoin (решение 125) для показа: снимок стакана N уровней (поток Book),
//! сделка /market/match (Trades), изменения /market/level2 (Diff) и снимок REST part orderbook (Snapshot).
//! Кто вызывает: tap.rs (decode), feed.rs (номера изменений), тесты.
//! От чего зависит: depth.rs, core/types, gateway/tap.rs.
//! Настройки: нет.
//!
//! Официально (спецификация SDK spec/ws/openapi-spot-public.json):
//! - Trade «/market/match:{symbol}» (subject trade.l3match): data {price, size, side, sequence, tradeId, time —
//!   наносекунды строкой, makerOrderId, takerOrderId}; side — сторона тейкера (в записи сторона из поля как есть).
//! - Orderbook - Increment «/market/level2:{symbol}» (subject trade.l2update): data {changes {asks, bids — [цена,
//!   количество, sequence]}, sequenceStart, sequenceEnd, symbol, time}; количество 0 — уровня нет.
//! - Get Part OrderBook GET /api/v1/market/orderbook/level2_{size} https://www.kucoin.com/docs-new/api-3470165 —
//!   {time, sequence, bids, asks}. Живые записи — tests/data/cex_samples_2026_11_04/.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{PriceLevel, TapDecoded, TapView, TradeTick};
use crate::gateway::tap::{TapData, TapStream};

use super::depth::{self, Msg};

/// Что делает: разбирает кадр окна. Принимает: поток, содержимое. Возвращает: TapDecoded. Ошибка — текст причины.
pub fn decode(stream: TapStream, data: &TapData) -> Result<TapDecoded, String> {
    let TapData::Text(t) = data else {
        return Err("двоичный кадр — данные KuCoin приходят текстом JSON".into());
    };
    if stream == TapStream::Snapshot {
        return snapshot(t);
    }
    match depth::parse(t) {
        Msg::Depth {
            bids,
            asks,
            exchange_time,
            ..
        } => Ok(TapDecoded {
            view: TapView::Book { bids, asks },
            exchange_ms: exchange_time.map(|t| t.timestamp_millis()),
        }),
        Msg::Bad { reason, .. } => Err(format!("стакан не разобран: {reason}")),
        Msg::Data { topic, data, .. } if topic.starts_with("/market/match:") => trade(&data),
        Msg::Data { topic, data, .. } if topic.starts_with("/market/level2:") => diff(&data),
        _ => Ok(TapDecoded {
            view: TapView::Other(t.to_string()),
            exchange_ms: None,
        }),
    }
}

/// Номера изменений кадра /market/level2: (sequenceStart, sequenceEnd). None — не такой кадр. Ошибок нет.
pub fn diff_range(t: &str) -> Option<(i64, i64)> {
    match depth::parse(t) {
        Msg::Data { topic, data, .. } if topic.starts_with("/market/level2:") => Some((
            data.get("sequenceStart")?.as_i64()?,
            data.get("sequenceEnd")?.as_i64()?,
        )),
        _ => None,
    }
}

/// Сделка. Ошибка — текст.
fn trade(d: &Value) -> Result<TapDecoded, String> {
    let s = |f: &str| d.get(f).and_then(Value::as_str).unwrap_or_default();
    let num =
        |f: &str| Decimal::from_str(s(f).trim()).map_err(|_| format!("{f} «{}» не число", s(f)));
    let time_ms = s("time")
        .parse::<i64>()
        .map(|ns| ns / 1_000_000)
        .unwrap_or_default();
    Ok(TapDecoded {
        view: TapView::Trades(vec![TradeTick {
            id: s("tradeId").to_string(),
            price: num("price")?,
            qty: num("size")?,
            taker: s("side").to_string(),
            time_ms,
        }]),
        exchange_ms: Some(time_ms),
    })
}

/// Изменения стакана (количество 0 сохраняется — «уровня нет»). Ошибка — текст.
fn diff(d: &Value) -> Result<TapDecoded, String> {
    let ch = d.get("changes").ok_or("нет changes")?;
    Ok(TapDecoded {
        view: TapView::Diff {
            bids: raw_levels(ch, "bids")?,
            asks: raw_levels(ch, "asks")?,
            full: false,
            from: d.get("sequenceStart").and_then(Value::as_i64),
            to: d.get("sequenceEnd").and_then(Value::as_i64),
        },
        exchange_ms: d.get("time").and_then(Value::as_i64),
    })
}

/// Снимок REST (как записан — тело ответа целиком или его data). Ошибка — текст.
fn snapshot(t: &str) -> Result<TapDecoded, String> {
    let v: Value = serde_json::from_str(t).map_err(|e| format!("не JSON: {e}"))?;
    let d = v.get("data").unwrap_or(&v);
    let seq = d
        .get("sequence")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok());
    Ok(TapDecoded {
        view: TapView::Diff {
            bids: raw_levels(d, "bids")?,
            asks: raw_levels(d, "asks")?,
            full: true,
            from: seq,
            to: seq,
        },
        exchange_ms: d.get("time").and_then(Value::as_i64),
    })
}

/// Уровни [цена, количество, …] без отбрасывания нулей. Ошибка — текст.
fn raw_levels(d: &Value, side: &str) -> Result<Vec<PriceLevel>, String> {
    let arr = d
        .get(side)
        .and_then(Value::as_array)
        .ok_or(format!("нет {side}"))?;
    arr.iter()
        .map(|l| {
            let p = l
                .as_array()
                .filter(|p| p.len() >= 2)
                .ok_or(format!("{side}: не [цена, количество]"))?;
            let n = |x: &Value| {
                x.as_str()
                    .and_then(|s| Decimal::from_str(s).ok())
                    .ok_or(format!("{side}: {x} не число"))
            };
            Ok(PriceLevel {
                price: n(&p[0])?,
                qty: n(&p[1])?,
            })
        })
        .collect()
}
