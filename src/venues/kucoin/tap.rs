//! Файл: venues/kucoin/tap.rs
//! Что делает: запись окна на KuCoin (решение 125): включить запись монеты (текстовые кадры её снимков стакана из
//! торговых соединений — ws_conn.rs; сделки и изменения стакана со снимком REST — свои соединения feed.rs), выключить
//! и разобрать записанный кадр (tap_decode.rs).
//! Кто вызывает: mod.rs (window_tap, window_untap, decode_tap).
//! От чего зависит: ctx.rs, symbols.rs, ws_conn.rs (тема стакана), feed.rs, tap_decode.rs, gateway/tap.rs.
//! Настройки: venue.depth_levels (тема стакана), venue.window_depth_limit [kucoin] (снимок для изменений: 20 или 100;
//! не задана — изменения стакана не пишутся).
//!
//! Официально: «/market/match:{symbol}» — «Subscribe to this topic to get Level 3 matching event data flows», по
//! сообщению на каждое исполнение (data — одна сделка с tradeId); «/market/level2:{symbol}» — «increment change orderbook
//! data (all depths)» (спецификация SDK spec/ws/openapi-spot-public.json).

use crate::core::types::{Asset, TapDecoded};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tap::{TapData, TapInfo, TapSink, TapStream};

use super::ctx::{Ctx, ID};
use super::feed::SNAPSHOT_SIZES;
use super::symbols::Symbols;

/// Что делает: включает запись монеты, ленту сделок и (если задана глубина снимка) изменения стакана.
/// Ошибки: NotConfigured (venue.depth_levels), BadResponse — пары нет в справочнике.
pub fn start(ctx: &Ctx, symbols: &Symbols, asset: &Asset, sink: TapSink) -> GatewayResult<TapInfo> {
    let symbol = symbols.get(ctx, &asset.ticker)?.symbol;
    let levels = ctx.int("venue.depth_levels")?;
    let stop = ctx.taps.add(&asset.ticker, sink);
    let open = |topic: String, kind: TapStream| {
        let (c, t, s) = (ctx.clone(), asset.ticker.clone(), symbol.clone());
        tokio::spawn(super::feed::run(c, t, s, topic.clone(), kind, stop.clone()));
        topic
    };
    let trades = open(format!("/market/match:{symbol}"), TapStream::Trades);
    let diff = match ctx.int("venue.window_depth_limit") {
        Ok(n) if SNAPSHOT_SIZES.contains(&n) => {
            Ok(open(format!("/market/level2:{symbol}"), TapStream::Diff))
        }
        Ok(n) => Err(format!(
            "venue.window_depth_limit = {n}: у KuCoin снимок только 20 или 100"
        )),
        Err(e) => Err(e.message),
    };
    Ok(TapInfo {
        book_channel: super::ws_conn::topic(levels, &[symbol.as_str()]),
        trades_channel: Ok(trades),
        trades_form: "по сообщению на каждое исполнение (/market/match, subject trade.l3match, data — одна сделка с \
                      tradeId); side — из поля как есть"
            .into(),
        diff_channel: diff,
    })
}

/// Что делает: выключает запись монеты (соединения окна отписываются и закрываются). Ошибок нет.
pub fn stop(ctx: &Ctx, asset: &Asset) {
    ctx.taps.remove(&asset.ticker);
}

/// Что делает: разбирает записанный кадр. Ошибки: BadResponse — причина.
pub fn decode(stream: TapStream, data: &TapData) -> GatewayResult<TapDecoded> {
    super::tap_decode::decode(stream, data).map_err(|m| {
        GatewayError::new(
            ErrorCode::BadResponse,
            Some(crate::core::types::VenueId(ID.into())),
            m,
        )
    })
}
