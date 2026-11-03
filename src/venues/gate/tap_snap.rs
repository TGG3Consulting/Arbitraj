//! Файл: venues/gate/tap_snap.rs
//! Что делает: изменения стакана Gate для окна (дополнение решения 125): номера кадров spot.order_book_update (U, u) —
//! когда нужен снимок REST (первый кадр сессии, разрыв номеров), сам снимок GET /spot/order_book?with_id=true (в окно
//! — ответ как есть, поток Snapshot) и разбор кадра изменений и снимка для показа.
//! Кто вызывает: feed.rs (Versions, take), tap.rs (decode_diff, decode_snapshot), тесты.
//! От чего зависит: rest.rs (public), ctx.rs, core/types.
//! Настройки: venue.window_depth_limit [gate] (limit снимка), venue.ws_reconnect_pause_ms (не чаще — повтор снимка).
//!
//! Официально (Spot WebSocket v4, spot.order_book_update, https://www.gate.com/docs/developers/apiv4/ws/en/): result —
//! t, s, U («first update ID»), u («last update ID»), b, a (количество «0» — уровня нет); порядок ведения локального
//! стакана: подписаться и копить; снимок REST с with_id=true — его id (baseID); применять с кадра, где
//! U <= baseID+1 <= u; кадры с u < baseID+1 — отбросить; кадр с U > baseID+1 — пропуск, снимок заново. REST GET
//! /spot/order_book (модель OrderBook): id («Valid only when with_id is set to true»), current, update, asks, bids.

use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

use crate::core::types::{TapDecoded, TapView};
use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};

/// Номера изменений стакана одной сессии.
#[derive(Debug, Default)]
pub struct Versions {
    /// Номер, до которого стакан известен (снимок или последний кадр); None — снимка ещё нет.
    cur: Option<i64>,
    /// Неудавшийся снимок — повтор не раньше этого.
    retry_at: Option<Instant>,
}

impl Versions {
    /// Что делает: кадр изменений пришёл — нужен ли снимок (снимка нет или разрыв номеров). Принимает: текст кадра.
    /// Ошибок нет (кадр не разобран или не изменения — снимок не нужен).
    pub fn need_snapshot(&mut self, text: &str) -> bool {
        let Some((from, to)) = ids(text) else {
            return false;
        };
        if self.retry_at.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        match self.cur {
            Some(c) if to <= c => false,
            Some(c) if from <= c + 1 => {
                self.cur = Some(to);
                false
            }
            _ => true,
        }
    }
}

/// U и u кадра spot.order_book_update. None — не такой кадр. Ошибок нет.
fn ids(text: &str) -> Option<(i64, i64)> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("channel")?.as_str()? != "spot.order_book_update"
        || v.get("event")?.as_str()? != "update"
    {
        return None;
    }
    let r = v.get("result")?;
    Some((r.get("U")?.as_i64()?, r.get("u")?.as_i64()?))
}

/// Что делает: снимок REST в окно (поток Snapshot), номер — id снимка. Ошибок нет — сбой в журнал, повтор не раньше
/// venue.ws_reconnect_pause_ms.
pub async fn take(ctx: &Ctx, ticker: &str, symbol: &str, conn: &str, ver: &mut Versions) {
    let r = match ctx.int("venue.window_depth_limit") {
        Ok(limit) => {
            let p = vec![
                ("currency_pair", symbol.to_string()),
                ("limit", limit.to_string()),
                ("with_id", "true".to_string()),
            ];
            super::rest::public(ctx, "/spot/order_book", p)
                .await
                .map_err(|e| e.message)
        }
        Err(e) => Err(e.message),
    };
    match r.and_then(|v| {
        v.get("id")
            .and_then(Value::as_i64)
            .map(|id| (id, v))
            .ok_or("снимок без id".to_string())
    }) {
        Ok((id, v)) => {
            ver.cur = Some(id);
            ver.retry_at = None;
            let text = v.to_string();
            ctx.taps
                .push(ID, ticker, TapStream::Snapshot, conn, now_ns(), || {
                    TapData::Text(text)
                });
        }
        Err(e) => {
            let pause = ctx.int("venue.ws_reconnect_pause_ms").unwrap_or_default();
            ver.retry_at = Some(Instant::now() + Duration::from_millis(pause));
            let m = format!("снимок стакана {symbol} для окна не получен: {e}");
            let _ = ctx
                .journal
                .write(Level::Warn, "venue.ws", Some(ID), m, None)
                .await;
        }
    }
}

/// Разбор кадра изменений для показа. Ошибка — текст причины.
pub fn decode_diff(text: &str) -> Result<TapDecoded, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("JSON: {e}"))?;
    let Some(r) = v
        .get("result")
        .filter(|_| v.get("event").and_then(Value::as_str) == Some("update"))
    else {
        return Ok(TapDecoded {
            view: TapView::Other("кадр не изменения стакана".into()),
            exchange_ms: None,
        });
    };
    Ok(TapDecoded {
        view: TapView::Diff {
            bids: all_levels(r, "b")?,
            asks: all_levels(r, "a")?,
            full: matches!(r.get("full"), Some(Value::Bool(true))),
            from: r.get("U").and_then(Value::as_i64),
            to: r.get("u").and_then(Value::as_i64),
        },
        exchange_ms: r.get("t").and_then(Value::as_i64),
    })
}

/// Разбор снимка REST для показа (полный стакан, номер — id). Ошибка — текст причины.
pub fn decode_snapshot(text: &str) -> Result<TapDecoded, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("JSON: {e}"))?;
    let id = v.get("id").and_then(Value::as_i64);
    Ok(TapDecoded {
        view: TapView::Diff {
            bids: all_levels(&v, "bids")?,
            asks: all_levels(&v, "asks")?,
            full: true,
            from: id,
            to: id,
        },
        exchange_ms: v.get("current").and_then(Value::as_i64),
    })
}

/// Уровни с нулевым количеством включительно (в изменениях 0 — «уровня больше нет»). Ошибка — текст причины.
fn all_levels(r: &Value, side: &str) -> Result<Vec<crate::core::types::PriceLevel>, String> {
    use std::str::FromStr;
    let arr = r
        .get(side)
        .and_then(Value::as_array)
        .ok_or(format!("нет {side}"))?;
    arr.iter()
        .map(|l| {
            let p = l
                .as_array()
                .filter(|p| p.len() >= 2)
                .ok_or(format!("{side}: уровень не [цена, количество]"))?;
            let n = |x: &Value| {
                x.as_str()
                    .and_then(|s| rust_decimal::Decimal::from_str(s).ok())
            };
            Ok(crate::core::types::PriceLevel {
                price: n(&p[0]).ok_or(format!("{side}: цена не число"))?,
                qty: n(&p[1]).ok_or(format!("{side}: количество не число"))?,
            })
        })
        .collect()
}
