//! Файл: venues/mexc/tap_snap.rs
//! Что делает: изменения стакана MEXC для окна (дополнение решения 125): версии кадров канала
//! spot@public.aggre.depth.v3.api.pb (fromVersion / toVersion) — когда нужен снимок REST (первый кадр сессии, разрыв
//! версий), сам снимок GET /api/v3/depth (в окно — ответ как есть, поток Snapshot) и разбор кадра изменений (тело 313)
//! и снимка для показа.
//! Кто вызывает: feed.rs (Versions, take), tap.rs (decode_diff, decode_snapshot), тесты.
//! От чего зависит: rest.rs (public), proto.rs, depth.rs (уровни), ctx.rs, core/types.
//! Настройки: venue.window_depth_limit [mexc] (limit снимка; официально «default 100; max 5000»), venue.ws_reconnect_pause_ms
//! (не чаще — повтор неудавшегося снимка).
//!
//! Официально «How to Properly Maintain a Local Copy of the Order Book»: (2) снимок /api/v3/depth?symbol&limit;
//! (3) «fromVersion of each new push message should be exactly equal to the toVersion + 1 of the previous message.
//! Otherwise, packet loss has occurred, and reinitialization from step 2 is required»; (5) toVersion меньше version
//! снимка — устаревшее; (6) fromVersion больше version снимка — пропуск, снимок заново; (4) количество — новое
//! значение уровня, 0 — уровня нет. Ответ /api/v3/depth (Market Data → Order Book): lastUpdateId, bids/asks
//! [цена, количество]; в процедуре он назван «version» — сопоставляется с lastUpdateId (в ответе другого поля нет).

use std::str::FromStr;
use std::time::Duration;

use prost::Message as _;
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::time::Instant;

use crate::core::types::{PriceLevel, TapDecoded, TapView};
use crate::gateway::tap::{TapData, TapStream, now_ns};
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::proto::{Body, DepthItem, Wrapper};

/// Версии изменений стакана одной сессии.
#[derive(Debug, Default)]
pub struct Versions {
    /// Версия, до которой стакан известен (снимок или последний кадр); None — снимка ещё нет.
    cur: Option<i64>,
    /// Неудавшийся снимок — повтор не раньше этого.
    retry_at: Option<Instant>,
}

impl Versions {
    /// Что делает: кадр изменений пришёл — нужен ли снимок (нет снимка или разрыв версий). Принимает: байты кадра.
    /// Ошибок нет (кадр не разобран — снимок не нужен).
    pub fn need_snapshot(&mut self, b: &[u8]) -> bool {
        let Some((from, to)) = Wrapper::decode(b).ok().and_then(|w| match w.body {
            Some(Body::AggreDepths(d)) => Some((
                d.from_version.parse::<i64>().ok()?,
                d.to_version.parse::<i64>().ok()?,
            )),
            _ => None,
        }) else {
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

/// Что делает: снимок REST в окно (поток Snapshot), версия — lastUpdateId. Ошибок нет — сбой в журнал, повтор
/// не раньше venue.ws_reconnect_pause_ms.
pub async fn take(ctx: &Ctx, ticker: &str, symbol: &str, conn: &str, ver: &mut Versions) {
    let r = match ctx.int("venue.window_depth_limit") {
        Ok(limit) => {
            let p = vec![("symbol", symbol.to_string()), ("limit", limit.to_string())];
            super::rest::public(ctx, "/api/v3/depth", p)
                .await
                .map_err(|e| e.message)
        }
        Err(e) => Err(e.message),
    };
    let rx_ns = now_ns();
    match r {
        Ok(v) => {
            ver.cur = v.get("lastUpdateId").and_then(Value::as_i64);
            ver.retry_at = None;
            let text = v.to_string();
            ctx.taps
                .push(ID, ticker, TapStream::Snapshot, conn, rx_ns, || {
                    TapData::Text(text)
                });
        }
        Err(e) => {
            let pause = ctx.int("venue.ws_reconnect_pause_ms").unwrap_or(0);
            ver.retry_at = Some(Instant::now() + Duration::from_millis(pause));
            let msg = format!("окно {ticker}: снимок стакана /api/v3/depth не получен: {e}");
            let _ = ctx
                .journal
                .write(Level::Warn, "venue.rest", Some(ID), msg, None)
                .await;
        }
    }
}

/// Что делает: разбирает кадр изменений (тело 313). Ошибки: текст причины.
pub fn decode_diff(b: &[u8]) -> Result<TapDecoded, String> {
    let w = Wrapper::decode(b).map_err(|e| format!("protobuf: {e}"))?;
    let exchange_ms = w.send_time.or(w.create_time);
    let Some(Body::AggreDepths(d)) = w.body else {
        return Ok(TapDecoded {
            view: TapView::Other(format!("кадр канала {} — не изменения стакана", w.channel)),
            exchange_ms,
        });
    };
    let lv = |items: &[DepthItem]| -> Result<Vec<PriceLevel>, String> {
        items.iter().map(|l| level(&l.price, &l.quantity)).collect()
    };
    Ok(TapDecoded {
        view: TapView::Diff {
            bids: lv(&d.bids)?,
            asks: lv(&d.asks)?,
            full: false,
            from: d.from_version.parse().ok(),
            to: d.to_version.parse().ok(),
        },
        exchange_ms,
    })
}

/// Что делает: разбирает записанный снимок REST. Ошибки: текст причины.
pub fn decode_snapshot(text: &str) -> Result<TapDecoded, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("не JSON: {e}"))?;
    let side = |k: &str| -> Result<Vec<PriceLevel>, String> {
        let arr = v
            .get(k)
            .and_then(Value::as_array)
            .ok_or(format!("нет {k}"))?;
        arr.iter()
            .map(|l| {
                let p = l
                    .as_array()
                    .filter(|p| p.len() >= 2)
                    .ok_or(format!("{k}: уровень не [цена, количество]"))?;
                level(
                    p[0].as_str().unwrap_or_default(),
                    p[1].as_str().unwrap_or_default(),
                )
            })
            .collect()
    };
    let id = v.get("lastUpdateId").and_then(Value::as_i64);
    Ok(TapDecoded {
        view: TapView::Diff {
            bids: side("bids")?,
            asks: side("asks")?,
            full: true,
            from: id,
            to: id,
        },
        exchange_ms: None,
    })
}

/// Уровень из строк цены и количества. Ошибка — текст причины.
fn level(p: &str, q: &str) -> Result<PriceLevel, String> {
    Ok(PriceLevel {
        price: Decimal::from_str(p.trim()).map_err(|_| format!("цена «{p}» не число"))?,
        qty: Decimal::from_str(q.trim()).map_err(|_| format!("количество «{q}» не число"))?,
    })
}
