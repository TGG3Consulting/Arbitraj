//! Файл: venues/gate/tap.rs
//! Что делает: запись окна на Gate (решение 125): включить запись монеты (кадры её стакана spot.order_book из торговых
//! соединений — ws_conn.rs; лента сделок spot.trades и изменения стакана spot.order_book_update со снимком REST — свои
//! соединения feed.rs, tap_snap.rs), выключить и разобрать записанный кадр для показа.
//! Кто вызывает: mod.rs (контракт: window_tap, window_untap, decode_tap), тесты (decode).
//! От чего зависит: ctx.rs, depth.rs, feed.rs, tap_snap.rs, ws_conn.rs (channel), gateway/tap.rs, core/types.
//! Настройки: venue.quote_asset, venue.depth_levels, venue.book_interval (имя канала стакана), venue.window_diff_interval
//! [gate] и venue.window_depth_limit (изменения стакана и снимок; не заданы — изменения не пишутся).
//!
//! Официально (Spot WebSocket v4, spot.trades): side — «buy/sell, taker side» («Only the taker side is notified»),
//! create_time_ms — время сделки (строка «…520.563000» — миллисекунды с дробью), id — ID сделки; одна сделка в
//! уведомлении.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Asset, TapDecoded, TapView, TradeTick, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tap::{TapData, TapInfo, TapSink, TapStream};

use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::feed::{DIFF_INTERVALS, Feed};

/// Что делает: включает запись монеты, её ленту сделок и изменения стакана (если заданы их настройки).
/// Принимает: окружение, монету, приёмник. Возвращает: каналы и вид ленты. Ошибки: NotConfigured.
pub fn start(ctx: &Ctx, asset: &Asset, sink: TapSink) -> GatewayResult<TapInfo> {
    let symbol = ctx.symbol(&asset.ticker)?;
    let levels = ctx.int("venue.depth_levels")?;
    let interval = ctx.text("venue.book_interval")?;
    let stop = ctx.taps.add(&asset.ticker, sink);
    let open = |feed: Feed, iv: String| {
        let (c, t, s) = (ctx.clone(), asset.ticker.clone(), symbol.clone());
        tokio::spawn(super::feed::run(c, t, s, feed, iv, stop.clone()));
        format!("{} {symbol}", feed.name())
    };
    let trades = Ok(open(Feed::Trades, String::new()));
    let diff = match (
        ctx.text("venue.window_diff_interval"),
        ctx.int("venue.window_depth_limit"),
    ) {
        (Ok(iv), Ok(_)) if DIFF_INTERVALS.contains(&iv.as_str()) => Ok(open(Feed::Diff, iv)),
        (Ok(iv), Ok(_)) => Err(format!(
            "venue.window_diff_interval = {iv}: у Gate только {DIFF_INTERVALS:?}"
        )),
        (Err(e), _) | (_, Err(e)) => Err(e.message),
    };
    Ok(TapInfo {
        book_channel: super::ws_conn::channel(&symbol, levels, &interval),
        trades_channel: trades,
        trades_form: "по одной сделке в кадре, только сторона тейкера (Spot WebSocket v4, spot.trades: «Only the \
                      taker side is notified»)"
            .into(),
        diff_channel: diff,
    })
}

/// Что делает: выключает запись монеты (соединения окна отписываются и закрываются). Ошибок нет.
pub fn stop(ctx: &Ctx, asset: &Asset) {
    ctx.taps.remove(&asset.ticker);
}

/// Что делает: разбирает записанный кадр. Принимает: поток, содержимое. Возвращает: TapDecoded.
/// Ошибки: BadResponse — не текстовый кадр или кадр не разобран.
pub fn decode(stream: TapStream, data: &TapData) -> GatewayResult<TapDecoded> {
    let TapData::Text(t) = data else {
        return Err(bad("двоичный кадр — данные Gate приходят текстом (JSON)"));
    };
    match stream {
        TapStream::Book => match depth::parse(t) {
            Msg::Depth {
                bids,
                asks,
                exchange_time,
                ..
            } => Ok(TapDecoded {
                view: TapView::Book { bids, asks },
                exchange_ms: exchange_time.map(|t| t.timestamp_millis()),
            }),
            Msg::Bad { reason, .. } => Err(bad(&format!("стакан не разобран: {reason}"))),
            _ => Ok(other("кадр не стакан")),
        },
        TapStream::Trades => trades(t),
        TapStream::Diff => super::tap_snap::decode_diff(t).map_err(|e| bad(&e)),
        TapStream::Snapshot => super::tap_snap::decode_snapshot(t).map_err(|e| bad(&e)),
    }
}

/// Сделка кадра spot.trades. Ошибки: BadResponse.
fn trades(t: &str) -> GatewayResult<TapDecoded> {
    let v: Value = serde_json::from_str(t).map_err(|e| bad(&format!("JSON: {e}")))?;
    let (Some(r), Some("update")) = (v.get("result"), v.get("event").and_then(Value::as_str))
    else {
        return Ok(other("кадр не сделка"));
    };
    let s = |f: &str| {
        r.get(f)
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| x.to_string())
            })
            .unwrap_or_default()
    };
    let num = |f: &str| {
        Decimal::from_str(s(f).trim()).map_err(|_| bad(&format!("{f} «{}» не число", s(f))))
    };
    let time_ms = s("create_time_ms")
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or_default();
    let tick = TradeTick {
        id: s("id"),
        price: num("price")?,
        qty: num("amount")?,
        taker: s("side"),
        time_ms,
    };
    Ok(TapDecoded {
        view: TapView::Trades(vec![tick]),
        exchange_ms: v.get("time_ms").and_then(Value::as_i64),
    })
}

/// Кадр не стакан и не сделки. Ошибок нет.
fn other(text: &str) -> TapDecoded {
    TapDecoded {
        view: TapView::Other(text.to_string()),
        exchange_ms: None,
    }
}

/// Ошибка разбора кадра Gate. Ошибок нет.
fn bad(msg: &str) -> GatewayError {
    GatewayError::new(ErrorCode::BadResponse, Some(VenueId(ID.into())), msg)
}
