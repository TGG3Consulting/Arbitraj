//! Файл: venues/bingx/tap.rs
//! Что делает: запись окна на BingX (решение 125): включить запись монеты (кадры её стакана из торговых соединений —
//! ws_conn.rs; лента сделок и изменения стакана всей глубины — свои соединения feed.rs), выключить и разобрать
//! записанный кадр для показа — стакан канала «<SYMBOL>@depth<N>» (depth.rs), сделку «<SYMBOL>@trade», изменения
//! «<SYMBOL>@incrDepth» (tap_diff.rs).
//! Кто вызывает: mod.rs (контракт: window_tap, window_untap, decode_tap), тесты (decode).
//! От чего зависит: ctx.rs, depth.rs, num.rs, feed.rs, tap_diff.rs, gateway/tap.rs, core/types.
//! Настройки: venue.quote_asset, venue.depth_levels (имя канала стакана); соединения сделок и изменений — feed.rs.
//!
//! Формат сделки — docs-v3, Spot → Websocket Market Data → «Subscription transaction by transaction» (пример
//! dataUpdateDemo): {"code":0,"data":{"E":время события,"T":время сделки,"e":"trade","m":buyer is maker,"p":цена,
//! "q":количество,"s":символ,"t":ID},"dataType":"BTC-USDT@trade","success":true,"timestamp":…}. «m: true — this trade
//! is a passive sell order» → активная продажа. Официальный справочник api-ai-skills (spot-ws-market/api-reference.md)
//! описывает data как «Array of trade records» — расхождение: принимаются оба вида (объект и массив).

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::core::types::{Asset, TapDecoded, TapView, TradeTick};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tap::{TapData, TapInfo, TapSink, TapStream};

use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::feed::Feed;
use super::num;

/// Что делает: включает запись монеты и запускает её ленту сделок. Принимает: окружение, монету, приёмник.
/// Возвращает: каналы и вид ленты. Ошибки: NotConfigured (venue.quote_asset, venue.depth_levels).
pub fn start(ctx: &Ctx, asset: &Asset, sink: TapSink) -> GatewayResult<TapInfo> {
    let symbol = ctx.symbol(&asset.ticker)?;
    let levels = ctx.int("venue.depth_levels")?;
    let stop = ctx.taps.add(&asset.ticker, sink);
    for feed in [Feed::Trades, Feed::Diff] {
        let (c, t, s) = (ctx.clone(), asset.ticker.clone(), symbol.clone());
        tokio::spawn(super::feed::run(c, t, s, feed, stop.clone()));
    }
    Ok(TapInfo {
        book_channel: format!("{symbol}@depth{levels}"),
        trades_channel: Ok(Feed::Trades.channel(&symbol)),
        trades_form:
            "по одной сделке в сообщении (docs-v3: «Subscription transaction by transaction»)"
                .into(),
        diff_channel: Ok(Feed::Diff.channel(&symbol)),
    })
}

/// Что делает: выключает запись монеты (лента сделок отписывается и закрывается). Ошибок нет.
pub fn stop(ctx: &Ctx, asset: &Asset) {
    ctx.taps.remove(&asset.ticker);
}

/// Что делает: разбирает записанный кадр. Принимает: поток, содержимое. Возвращает: TapDecoded.
/// Ошибки: BadResponse — кадр не текст или не разобран (причина).
pub fn decode(stream: TapStream, data: &TapData) -> GatewayResult<TapDecoded> {
    let TapData::Text(text) = data else {
        return Err(bad("двоичный кадр — у BingX кадры текстовые (после GZIP)"));
    };
    match stream {
        TapStream::Book => match depth::parse(text) {
            Msg::Depth {
                bids,
                asks,
                exchange_time,
                ..
            } => Ok(TapDecoded {
                view: TapView::Book { bids, asks },
                exchange_ms: exchange_time.map(|t: DateTime<Utc>| t.timestamp_millis()),
            }),
            Msg::BadDepth { reason, .. } => Err(bad(&format!("стакан не разобран: {reason}"))),
            _ => Ok(other(text)),
        },
        TapStream::Trades => trades(text),
        TapStream::Diff => super::tap_diff::decode(text).map_err(|e| bad(&e)),
        TapStream::Snapshot => Err(bad(
            "у BingX снимок стакана приходит в канале @incrDepth (action «all»)",
        )),
    }
}

/// Сделки кадра канала @trade (см. шапку). Ошибки: BadResponse.
fn trades(text: &str) -> GatewayResult<TapDecoded> {
    let v: Value = serde_json::from_str(text).map_err(|e| bad(&format!("не JSON: {e}")))?;
    let items: Vec<&Value> = match v.get("data") {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(d @ Value::Object(_)) => vec![d],
        _ => return Ok(other(text)),
    };
    let list = items
        .into_iter()
        .map(tick)
        .collect::<Result<Vec<_>, String>>()
        .map_err(|e| bad(&e))?;
    let exchange_ms = v
        .get("data")
        .and_then(|d| d.get("E"))
        .and_then(Value::as_i64)
        .or_else(|| v.get("timestamp").and_then(Value::as_i64));
    Ok(TapDecoded {
        view: TapView::Trades(list),
        exchange_ms,
    })
}

/// Одна сделка {t, p, q, T, m}. Ошибка — текст причины.
fn tick(d: &Value) -> Result<TradeTick, String> {
    let field = |k: &str| d.get(k).ok_or(format!("нет поля {k}"));
    let id = match field("t")? {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let time_ms = match field("T")? {
        Value::String(s) => s.parse().map_err(|_| "T не число".to_string())?,
        v => v.as_i64().ok_or("T не число")?,
    };
    let maker = field("m")?.as_bool().ok_or("m не да/нет")?;
    Ok(TradeTick {
        id,
        price: num::to_dec(field("p")?).ok_or("p не число")?,
        qty: num::to_dec(field("q")?).ok_or("q не число")?,
        taker: if maker { "sell" } else { "buy" }.into(),
        time_ms,
    })
}

/// Кадр не стакан и не сделки. Ошибок нет.
fn other(text: &str) -> TapDecoded {
    TapDecoded {
        view: TapView::Other(text.to_string()),
        exchange_ms: None,
    }
}

/// Ошибка разбора кадра BingX. Ошибок нет.
fn bad(msg: &str) -> GatewayError {
    GatewayError::new(
        ErrorCode::BadResponse,
        Some(crate::core::types::VenueId(ID.into())),
        msg,
    )
}
