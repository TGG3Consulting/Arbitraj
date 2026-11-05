//! Файл: venues/binance/tap.rs
//! Что делает: запись окна на Binance (решение 125): включить запись монеты (кадры её частичного стакана из торговых
//! соединений — ws_conn.rs; сделки и изменения стакана со снимком REST — свои соединения feed.rs), выключить и
//! разобрать записанный кадр (tap_decode.rs).
//! Кто вызывает: mod.rs (window_tap, window_untap, decode_tap).
//! От чего зависит: ctx.rs, symbols.rs, ws.rs (config), ws_conn.rs (имя потока), feed.rs, tap_decode.rs, gateway/tap.rs.
//! Настройки: venue.depth_levels, venue.book_interval (имена потоков); venue.window_depth_limit [binance] (снимок для
//! изменений стакана, 1..5000; не задана — изменения стакана не пишутся).
//!
//! Официально (web-socket-streams): Trade Streams «<symbol>@trade» — «Raw trade information; each trade has a unique
//! buyer and seller» (по кадру на сделку); Diff. Depth Stream «<symbol>@depth» или «<symbol>@depth@100ms».

use crate::core::types::{Asset, TapDecoded};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::tap::{TapData, TapInfo, TapSink, TapStream};

use super::ctx::{Ctx, ID};
use super::feed::MAX_SNAPSHOT;
use super::symbols::Symbols;

/// Что делает: включает запись монеты, ленту сделок и (если задан снимок) изменения стакана.
/// Ошибки: NotConfigured (настройки потоков), BadResponse — пары нет в справочнике.
pub fn start(ctx: &Ctx, symbols: &Symbols, asset: &Asset, sink: TapSink) -> GatewayResult<TapInfo> {
    let symbol = symbols.get(ctx, &asset.ticker)?.symbol;
    let cfg = super::ws::config(ctx)
        .map_err(|m| ctx.tag(GatewayError::new(ErrorCode::NotConfigured, None, m)))?;
    let lower = symbol.to_lowercase();
    let stop = ctx.taps.add(&asset.ticker, sink);
    let open = |stream: String, kind: TapStream| {
        let (c, t, s) = (ctx.clone(), asset.ticker.clone(), symbol.clone());
        tokio::spawn(super::feed::run(
            c,
            t,
            s,
            stream.clone(),
            kind,
            stop.clone(),
        ));
        stream
    };
    let trades = open(format!("{lower}@trade"), TapStream::Trades);
    let diff = match ctx.int("venue.window_depth_limit") {
        Ok(n) if (1..=MAX_SNAPSHOT).contains(&n) => Ok(open(
            format!("{lower}@depth{}", cfg.suffix),
            TapStream::Diff,
        )),
        Ok(n) => Err(format!(
            "venue.window_depth_limit = {n}: у Binance снимок от 1 до {MAX_SNAPSHOT}"
        )),
        Err(e) => Err(e.message),
    };
    Ok(TapInfo {
        book_channel: super::ws_conn::stream(&symbol, cfg.levels, cfg.suffix),
        trades_channel: Ok(trades),
        trades_form:
            "по кадру на сделку (<symbol>@trade, t — id сделки); тейкер — продавец, если m = true \
                      («Is the buyer the market maker?»)"
                .into(),
        diff_channel: diff,
    })
}

/// Что делает: выключает запись монеты (соединения окна закрываются). Ошибок нет.
pub fn stop(ctx: &Ctx, asset: &Asset) {
    ctx.taps.remove(&asset.ticker);
}

/// Что делает: разбирает записанный кадр. Ошибка BadResponse — причина.
pub fn decode(stream: TapStream, data: &TapData) -> GatewayResult<TapDecoded> {
    super::tap_decode::decode(stream, data).map_err(|m| {
        GatewayError::new(
            ErrorCode::BadResponse,
            Some(crate::core::types::VenueId(ID.into())),
            m,
        )
    })
}
