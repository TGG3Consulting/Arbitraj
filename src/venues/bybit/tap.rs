//! Файл: venues/bybit/tap.rs
//! Что делает: запись окна на Bybit (решение 125): включить запись монеты (кадры её стакана из торговых соединений —
//! ws_conn.rs; лента сделок publicTrade и стакан глубины venue.window_depth_limit — свои соединения feed.rs), выключить и
//! разобрать записанный кадр для показа.
//! Кто вызывает: mod.rs (контракт: window_tap, window_untap, decode_tap), тесты (decode).
//! От чего зависит: ctx.rs, depth.rs, num.rs, feed.rs, market.rs (DEPTH_LEVELS), gateway/tap.rs, core/types.
//! Настройки: venue.quote_asset, venue.depth_levels (канал стакана), venue.window_depth_limit [bybit] (глубина канала
//! изменений окна: 1, 50, 200, 1000; не задана — изменения не пишутся).
//!
//! Разбор (официальная документация):
//! - стакан торгового соединения (поток Book) — снимок → TapView::Book; изменение → TapView::Other со сводкой: в общем
//!   стакане окна (core/window_book.rs) изменения применяются только из потока Diff, чтобы глубина N и глубина окна
//!   не смешивались;
//! - поток Diff — снимок → Diff{full}, изменение → Diff с to = u (from не ставится: что u растёт строго на 1, в
//!   документации не сказано — Orderbook);
//! - сделки (Trade): data[] — i (Trade ID), p, v, S («Side of taker. Buy, Sell»), T (время сделки, мс); ts — кадра.

use serde_json::Value;

use crate::core::types::{Asset, TapDecoded, TapView, TradeTick, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tap::{TapData, TapInfo, TapSink, TapStream};

use super::ctx::{Ctx, ID};
use super::depth::{self, Msg, channel, int_of};
use super::num;

/// Включает запись монеты, её ленту сделок и стакан окна. Ошибки: NotConfigured (venue.quote_asset, venue.depth_levels).
pub fn start(ctx: &Ctx, asset: &Asset, sink: TapSink) -> GatewayResult<TapInfo> {
    let symbol = ctx.symbol(&asset.ticker)?;
    let levels = ctx.int("venue.depth_levels")?;
    let stop = ctx.taps.add(&asset.ticker, sink);
    let open = |ch: String, kind: TapStream| {
        let (c, t) = (ctx.clone(), asset.ticker.clone());
        tokio::spawn(super::feed::run(c, t, ch.clone(), kind, stop.clone()));
        ch
    };
    let trades = Ok(open(format!("publicTrade.{symbol}"), TapStream::Trades));
    let diff = match ctx.int("venue.window_depth_limit") {
        Ok(d) if super::market::DEPTH_LEVELS.contains(&d) => {
            Ok(open(channel(d, &symbol), TapStream::Diff))
        }
        Ok(d) => Err(format!(
            "venue.window_depth_limit = {d}: у Bybit глубины {:?}",
            super::market::DEPTH_LEVELS
        )),
        Err(e) => Err(e.message),
    };
    Ok(TapInfo {
        book_channel: channel(levels, &symbol),
        trades_channel: trades,
        trades_form: "сделки пачкой в кадре (Trade: «a single message may have up to 1024 trades»), каждая с i".into(),
        diff_channel: diff,
    })
}

/// Выключает запись монеты (соединения окна отписываются и закрываются). Ошибок нет.
pub fn stop(ctx: &Ctx, asset: &Asset) {
    ctx.taps.remove(&asset.ticker);
}

/// Разбирает записанный кадр. Ошибки: BadResponse — не текст или не разобран.
pub fn decode(stream: TapStream, data: &TapData) -> GatewayResult<TapDecoded> {
    let TapData::Text(text) = data else {
        return Err(bad("двоичный кадр — у Bybit кадры текстовые (JSON)"));
    };
    match stream {
        TapStream::Trades => trades(text),
        TapStream::Book | TapStream::Diff => match depth::parse(text) {
            Msg::Book {
                snapshot,
                u,
                ts,
                bids,
                asks,
                ..
            } => {
                let (b, a) = (levels(&bids)?, levels(&asks)?);
                let view = match (stream, snapshot) {
                    (TapStream::Book, true) => TapView::Book { bids: b, asks: a },
                    (TapStream::Book, false) => TapView::Other(format!(
                        "изменение стакана u={u}: {} bids, {} asks",
                        b.len(),
                        a.len()
                    )),
                    (_, full) => TapView::Diff {
                        bids: b,
                        asks: a,
                        full,
                        from: None,
                        to: Some(u),
                    },
                };
                Ok(TapDecoded {
                    view,
                    exchange_ms: ts,
                })
            }
            Msg::Bad(r) => Err(bad(&format!("стакан не разобран: {r}"))),
            _ => Ok(other(text)),
        },
        TapStream::Snapshot => Err(bad(
            "у Bybit снимок стакана приходит в канале orderbook (type snapshot)",
        )),
    }
}

/// Уровни строками → PriceLevel. Ошибка BadResponse.
fn levels(l: &[[&str; 2]]) -> GatewayResult<Vec<crate::core::types::PriceLevel>> {
    l.iter()
        .map(|[p, q]| match (num::str_dec(p), num::str_dec(q)) {
            (Some(price), Some(qty)) => Ok(crate::core::types::PriceLevel { price, qty }),
            _ => Err(bad(&format!("уровень [{p}, {q}] не число"))),
        })
        .collect()
}

/// Сделки кадра publicTrade. Ошибки: BadResponse.
fn trades(text: &str) -> GatewayResult<TapDecoded> {
    let v: Value = serde_json::from_str(text).map_err(|e| bad(&format!("не JSON: {e}")))?;
    let Some(items) = v.get("data").and_then(Value::as_array) else {
        return Ok(other(text));
    };
    let mut list = Vec::with_capacity(items.len());
    for d in items {
        let f = |k: &str| d.get(k).ok_or_else(|| bad(&format!("сделка без поля {k}")));
        list.push(TradeTick {
            id: num::text_opt(d, "i").unwrap_or_default(),
            price: num::to_dec(f("p")?).ok_or_else(|| bad("p не число"))?,
            qty: num::to_dec(f("v")?).ok_or_else(|| bad("v не число"))?,
            taker: match f("S")?.as_str() {
                Some("Buy") => "buy".into(),
                Some("Sell") => "sell".into(),
                other => format!("S {other:?}"),
            },
            time_ms: int_of(f("T")?).ok_or_else(|| bad("T не число"))?,
        });
    }
    Ok(TapDecoded {
        view: TapView::Trades(list),
        exchange_ms: v.get("ts").and_then(int_of),
    })
}

/// Кадр не стакан и не сделки. Ошибок нет.
fn other(text: &str) -> TapDecoded {
    TapDecoded {
        view: TapView::Other(text.to_string()),
        exchange_ms: None,
    }
}

/// Ошибка разбора кадра Bybit. Ошибок нет.
fn bad(msg: &str) -> GatewayError {
    GatewayError::new(ErrorCode::BadResponse, Some(VenueId(ID.into())), msg)
}
