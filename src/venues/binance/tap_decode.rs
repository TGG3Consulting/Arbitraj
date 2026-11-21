//! Файл: venues/binance/tap_decode.rs
//! Что делает: разбор записанного кадра окна Binance (решение 125) для показа: снимок частичного стакана (поток Book —
//! кадр комбинированного потока), сделка <symbol>@trade (Trades), изменения <symbol>@depth (Diff) и снимок REST
//! /api/v3/depth (Snapshot).
//! Кто вызывает: tap.rs (decode), feed.rs (номера изменений), тесты.
//! От чего зависит: depth.rs, core/types, gateway/tap.rs.
//! Настройки: нет.
//!
//! Официально (web-socket-streams): Trade Streams {"e":"trade","E","s","t" id,"p","q","T" время,"m" — «Is the buyer
//! the market maker?»} — тейкер продавец, если m = true; Diff. Depth Stream {"e":"depthUpdate","E","s","U" первый id,
//! "u" последний id,"b","a"} — количество 0 — уровня нет; снимок GET /api/v3/depth — {lastUpdateId, bids, asks}.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{PriceLevel, TapDecoded, TapView, TradeTick};
use crate::gateway::tap::{TapData, TapStream};

use super::depth::{self, Msg};

/// Что делает: разбирает кадр окна. Принимает: поток, содержимое. Возвращает: TapDecoded. Ошибка — текст.
pub fn decode(stream: TapStream, data: &TapData) -> Result<TapDecoded, String> {
    let TapData::Text(t) = data else {
        return Err("двоичный кадр — данные Binance приходят текстом JSON".into());
    };
    if stream == TapStream::Book {
        return match depth::parse(t) {
            Msg::Depth { book, .. } => {
                let (bids, asks) = depth::levels(&book)?;
                Ok(TapDecoded {
                    view: TapView::Book { bids, asks },
                    exchange_ms: None,
                })
            }
            _ => Ok(other(t)),
        };
    }
    let v: Value = serde_json::from_str(t).map_err(|e| format!("не JSON: {e}"))?;
    match (stream, v.get("e").and_then(Value::as_str)) {
        (TapStream::Snapshot, _) => snapshot(&v),
        (_, Some("trade")) => trade(&v),
        (_, Some("depthUpdate")) => diff(&v),
        _ => Ok(other(t)),
    }
}

/// Номера изменений кадра depthUpdate: (U, u). None — не такой кадр. Ошибок нет.
pub fn diff_range(t: &str) -> Option<(i64, i64)> {
    let v: Value = serde_json::from_str(t).ok()?;
    (v.get("e")?.as_str()? == "depthUpdate")
        .then(|| Some((v.get("U")?.as_i64()?, v.get("u")?.as_i64()?)))
        .flatten()
}

/// lastUpdateId снимка REST. None — нет поля. Ошибок нет.
pub fn snapshot_id(v: &Value) -> Option<i64> {
    v.get("lastUpdateId")?.as_i64()
}

/// Кадр не стакан и не сделка. Ошибок нет.
fn other(t: &str) -> TapDecoded {
    TapDecoded {
        view: TapView::Other(t.to_string()),
        exchange_ms: None,
    }
}

/// Сделка. Ошибка — текст.
fn trade(v: &Value) -> Result<TapDecoded, String> {
    let s = |f: &str| v.get(f).and_then(Value::as_str).unwrap_or_default();
    let n = |f: &str| Decimal::from_str(s(f)).map_err(|_| format!("{f} «{}» не число", s(f)));
    let time_ms = v.get("T").and_then(Value::as_i64).unwrap_or_default();
    let maker_buyer = v.get("m").and_then(Value::as_bool) == Some(true);
    Ok(TapDecoded {
        view: TapView::Trades(vec![TradeTick {
            id: v.get("t").map(Value::to_string).unwrap_or_default(),
            price: n("p")?,
            qty: n("q")?,
            taker: (if maker_buyer { "sell" } else { "buy" }).into(),
            time_ms,
        }]),
        exchange_ms: Some(time_ms),
    })
}

/// Изменения стакана (количество 0 сохраняется — «уровня нет»). Ошибка — текст.
fn diff(v: &Value) -> Result<TapDecoded, String> {
    Ok(TapDecoded {
        view: TapView::Diff {
            bids: raw_levels(v, "b")?,
            asks: raw_levels(v, "a")?,
            full: false,
            from: v.get("U").and_then(Value::as_i64),
            to: v.get("u").and_then(Value::as_i64),
        },
        exchange_ms: v.get("E").and_then(Value::as_i64),
    })
}

/// Снимок REST. Ошибка — текст.
fn snapshot(v: &Value) -> Result<TapDecoded, String> {
    let id = snapshot_id(v);
    Ok(TapDecoded {
        view: TapView::Diff {
            bids: raw_levels(v, "bids")?,
            asks: raw_levels(v, "asks")?,
            full: true,
            from: id,
            to: id,
        },
        exchange_ms: None,
    })
}

/// Уровни [цена, количество] без отбрасывания нулей. Ошибка — текст.
fn raw_levels(v: &Value, side: &str) -> Result<Vec<PriceLevel>, String> {
    let arr = v
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
