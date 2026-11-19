//! Файл: venues/kucoin/ws_fast.rs
//! Что делает: решение 423 — быстрый стакан пары KuCoin со страховкой снимками. Изменения /market/level2 → локальный
//! стакан (local_book.rs) по процедуре калибровки KuCoin со снимком Get Full OrderBook из общей очереди
//! (full_book.rs). Синхронизирован — ядру отдаётся он (верхние venue.depth_levels уровней, время биржи — time кадра).
//! Разрыв номеров, расхождение со снимком (fast_check.rs), переподписка, отказ подписки темы изменений, сбой снимка →
//! пара сразу на снимках: ядру — последний кадр /spotMarket/level2Depth5|50, если он не старше venue.book_stale_ms и не
//! старее уже отданного (иначе стакан пары снимается до следующего кадра снимков), затем новый снимок REST не раньше
//! venue.ws_reconnect_pause_ms; после синхронизации — снова быстрый. Каждое событие — счётчик и строка журнала
//! (fast_log.rs).
//! Кто вызывает: ws_sess.rs (on_incr, reconcile, fallback), ws_conn.rs (resubscribed), ws_full.rs (after).
//! От чего зависит: ws_sess.rs (Sess), fast_state.rs (Fast), local_book.rs, incr.rs, ws_full.rs (снимок REST),
//! fast_check.rs, fast_log.rs, ws.rs.
//! Настройки: venue.book_stale_ms [kucoin] (возраст снимка-запаса), остальные — ws.rs (Cfg.fast), full_book.rs.
//!
//! Официально: Orderbook - Increment https://www.kucoin.com/docs-new/3470068w0 (калибровка — local_book.rs), Get Full
//! OrderBook https://www.kucoin.com/docs-new/rest/spot-trading/market-data/get-full-orderbook.

use chrono::TimeZone;
use serde_json::Value;
use tokio::time::Instant;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::feed_health::BookPrint;
use crate::storage::journal::Level;

use super::fast_check::Verdict;
use super::fast_log::{self, Ev};
use super::incr;
use super::local_book::Applied;
use super::ws::{End, invalidate};
use super::ws_sess::Sess;

impl<'a> Sess<'a> {
    /// Кадр изменений (data). Возвращает: Some — сессия кончается. Ошибок нет (неразборчивый — пара на снимках).
    pub async fn on_incr(&mut self, data: &Value) -> Option<End> {
        self.fast.as_ref()?;
        let (symbol, u) = match incr::parse(data) {
            Ok(x) => x,
            Err((sym, why)) => {
                let m = format!("WebSocket {}: кадр изменений не разобран: {why}", self.name);
                self.ctx.log(Level::Error, "venue.ws", m).await;
                let a = sym
                    .as_deref()
                    .and_then(|s| self.by_symbol.get(s).copied())?;
                return self.fallback(sym.as_deref()?, a, &why).await;
            }
        };
        let asset = self.by_symbol.get(symbol.as_str()).copied()?;
        self.feed.frame(&asset.ticker);
        let f = self.fast.as_mut()?;
        let applied = f.local.entry(symbol.clone()).or_default().apply(u);
        self.after(&symbol, asset, applied).await
    }

    /// Сторож пар переподписал пачку (обе темы): изменения могли потеряться — пары пачки на снимках, стакан заново.
    /// Принимает: символы пачки. Возвращает: как on_incr.
    pub async fn resubscribed(&mut self, chunk: &[&'a str]) -> Option<End> {
        for s in chunk {
            let Some(a) = self.by_symbol.get(s).copied() else {
                continue;
            };
            if let Some(end) = self.fallback(s, a, "переподписка сторожем пар").await
            {
                return Some(end);
            }
        }
        None
    }

    /// Кадр снимков быстрой пары: сверка с локальным стаканом. Возвращает: как on_incr.
    pub async fn reconcile(
        &mut self,
        symbol: &str,
        asset: &'a Asset,
        snap: &OrderBook,
    ) -> Option<End> {
        let at = snap.exchange_time?.timestamp_millis();
        let f = self.fast.as_mut()?;
        let local = f.local.get(symbol).and_then(|l| l.time());
        let print = BookPrint::top(&snap.bids, &snap.asks, self.levels);
        let check = f.check.entry(symbol.to_string()).or_default();
        match check.on_snapshot(at, print, local) {
            Verdict::Diverge(why) => self.diverged(symbol, asset, &why).await,
            _ => None,
        }
    }

    /// Что после кадра или снимка. Возвращает: как on_incr.
    pub(super) async fn after(
        &mut self,
        symbol: &str,
        asset: &'a Asset,
        a: Applied,
    ) -> Option<End> {
        match a {
            Applied::Changed => self.fresh(symbol, asset).await,
            Applied::Repeat => None,
            Applied::Waiting => {
                self.want_full(symbol, asset).await;
                None
            }
            Applied::Gap(why) => self.fallback(symbol, asset, &why).await,
        }
    }

    /// Стакан изменился: сверка с ждущим снимком, затем ядру, если время известно и не старее отданного.
    async fn fresh(&mut self, symbol: &str, asset: &'a Asset) -> Option<End> {
        let levels = self.levels;
        let f = self.fast.as_mut()?;
        let lb = f.local.get(symbol)?;
        let time = lb.time()?;
        let (bids, asks) = lb.top(levels);
        let print = BookPrint {
            bids: bids.clone(),
            asks: asks.clone(),
        };
        let check = f.check.entry(symbol.to_string()).or_default();
        if let Verdict::Diverge(why) = check.on_local(time, print) {
            return self.diverged(symbol, asset, &why).await;
        }
        let book = OrderBook {
            venue: self.ctx.venue(),
            asset: asset.clone(),
            bids,
            asks,
            exchange_time: chrono::Utc.timestamp_millis_opt(time).single(),
            received_at: chrono::Utc::now(),
        };
        if !self.newer(symbol, &book) {
            return None;
        }
        let f = self.fast.as_mut()?;
        if f.on.insert(symbol.to_string()) {
            fast_log::note(
                self.ctx,
                Ev::ToFast,
                &asset.ticker,
                "стакан синхронизирован",
            );
        }
        self.give(symbol, book)
    }

    /// Расхождение со снимком: счётчик и переход на снимки. Возвращает: как on_incr.
    async fn diverged(&mut self, symbol: &str, asset: &'a Asset, why: &str) -> Option<End> {
        fast_log::note(self.ctx, Ev::Diverge, &asset.ticker, why);
        self.fallback(symbol, asset, why).await
    }

    /// Что делает: пара на снимках: локальный стакан сброшен, новый снимок REST не раньше паузы; если ядру шёл быстрый
    /// стакан — сразу последний кадр снимков (не старше venue.book_stale_ms и не старее отданного) или снятие стакана.
    /// Принимает: символ, монету, причину. Возвращает: как on_incr.
    pub async fn fallback(&mut self, symbol: &str, asset: &'a Asset, why: &str) -> Option<End> {
        let f = self.fast.as_mut()?;
        let was = f.on.remove(symbol);
        f.local
            .entry(symbol.to_string())
            .or_default()
            .reset(why.to_string());
        f.check.remove(symbol);
        f.not_before
            .insert(symbol.to_string(), Instant::now() + f.pause);
        let snap = f.snap.get(symbol).cloned();
        let ev = if was { Ev::ToSnap } else { Ev::ResyncFail };
        fast_log::note(self.ctx, ev, &asset.ticker, why);
        if !was {
            return None;
        }
        let max_age = self.ctx.int("venue.book_stale_ms").ok().map(|v| v as i64);
        let usable = snap.filter(|b| {
            let age = (chrono::Utc::now() - b.received_at).num_milliseconds();
            max_age.is_some_and(|m| age <= m) && self.newer(symbol, b)
        });
        match usable {
            Some(b) => self.give(symbol, b),
            None => {
                invalidate(self.ctx, self.books, self.out, &asset.ticker);
                None
            }
        }
    }
}
