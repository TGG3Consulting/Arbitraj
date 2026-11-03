//! Файл: venues/mexc/tap.rs
//! Что делает: запись окна на MEXC (решение 125): включить запись монеты (двоичные кадры её стакана из торговых
//! соединений — ws_conn.rs, лента сделок — своё соединение trades.rs), выключить и разобрать записанный кадр для
//! показа — частичный стакан (тело 303, depth.rs) и сделки (тело 314 PublicAggreDealsV3Api, proto.rs).
//! Кто вызывает: mod.rs (контракт: window_tap, window_untap, decode_tap), тесты (decode).
//! От чего зависит: ctx.rs, depth.rs, proto.rs, trades.rs, ws_conn.rs (channel), gateway/tap.rs, core/types.
//! Настройки: venue.quote_asset, venue.depth_levels (имя канала стакана), venue.trades_interval (канал сделок).
//!
//! Официально (Trade Streams): tradeType «1: Buy, 2: Sell» — сторона агрессора; time — время сделки; sendTime
//! обёртки — «Event time». Лента приходит кадрами за интервал (10 или 100 мс), в кадре — каждая сделка отдельно с
//! tradeId (по одной сделке «for each fill»), а не сделки, сложенные в одну. Нет venue.trades_interval — сделки не
//! пишутся (причина — на странице окна), стакан пишется.

use std::str::FromStr;

use prost::Message as _;
use rust_decimal::Decimal;

use crate::core::types::{Asset, TapDecoded, TapView, TradeTick, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tap::{TapData, TapInfo, TapSink, TapStream};

use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::proto::{Body, Wrapper};

/// Что делает: включает запись монеты и (если задан интервал) её ленту сделок. Принимает: окружение, монету,
/// приёмник. Возвращает: каналы и вид ленты. Ошибки: NotConfigured (venue.quote_asset, venue.depth_levels).
pub fn start(ctx: &Ctx, asset: &Asset, sink: TapSink) -> GatewayResult<TapInfo> {
    let symbol = ctx.symbol(&asset.ticker)?;
    let levels = ctx.int("venue.depth_levels")?;
    let stop = ctx.taps.add(&asset.ticker, sink);
    let trades = ctx
        .text("venue.trades_interval")
        .map(|i| super::trades::channel(&i, &symbol))
        .map_err(|e| e.message);
    if let Ok(ch) = &trades {
        tokio::spawn(super::trades::run(
            ctx.clone(),
            asset.ticker.clone(),
            ch.clone(),
            stop,
        ));
    }
    Ok(TapInfo {
        book_channel: super::ws_conn::channel(&symbol, levels),
        trades_channel: trades,
        trades_form: "кадр за интервал 10/100 мс, в нём каждая сделка отдельно с tradeId (Trade Streams: «raw trade \
                      information for each fill»; в имени канала — «aggre»)"
            .into(),
    })
}

/// Что делает: выключает запись монеты (лента сделок отписывается и закрывается). Ошибок нет.
pub fn stop(ctx: &Ctx, asset: &Asset) {
    ctx.taps.remove(&asset.ticker);
}

/// Что делает: разбирает записанный кадр. Принимает: поток, содержимое. Возвращает: TapDecoded.
/// Ошибки: BadResponse — не двоичный кадр или protobuf не разобран.
pub fn decode(stream: TapStream, data: &TapData) -> GatewayResult<TapDecoded> {
    let TapData::Binary(b) = data else {
        return Err(bad(
            "текстовый кадр — данные MEXC приходят двоичными (protobuf)",
        ));
    };
    match stream {
        TapStream::Book => match depth::parse_binary(b) {
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
        TapStream::Trades => trades(b),
    }
}

/// Сделки кадра (тело 314). Ошибки: BadResponse.
fn trades(b: &[u8]) -> GatewayResult<TapDecoded> {
    let w = Wrapper::decode(b).map_err(|e| bad(&format!("protobuf: {e}")))?;
    let exchange_ms = w.send_time.or(w.create_time);
    let Some(Body::AggreDeals(d)) = w.body else {
        return Ok(other(&format!("кадр канала {} — не сделки", w.channel)));
    };
    let num = |s: &str, what: &str| {
        Decimal::from_str(s.trim()).map_err(|_| bad(&format!("{what} «{s}» не число")))
    };
    let mut list = Vec::with_capacity(d.deals.len());
    for t in &d.deals {
        list.push(TradeTick {
            id: t.trade_id.clone(),
            price: num(&t.price, "цена")?,
            qty: num(&t.quantity, "количество")?,
            taker: match t.trade_type {
                1 => "buy".into(),
                2 => "sell".into(),
                n => format!("tradeType {n}"),
            },
            time_ms: t.time,
        });
    }
    Ok(TapDecoded {
        view: TapView::Trades(list),
        exchange_ms,
    })
}

/// Кадр не стакан и не сделки. Ошибок нет.
fn other(text: &str) -> TapDecoded {
    TapDecoded {
        view: TapView::Other(text.to_string()),
        exchange_ms: None,
    }
}

/// Ошибка разбора кадра MEXC. Ошибок нет.
fn bad(msg: &str) -> GatewayError {
    GatewayError::new(ErrorCode::BadResponse, Some(VenueId(ID.into())), msg)
}
