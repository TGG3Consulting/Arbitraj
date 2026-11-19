//! Файл: venues/kucoin/ws_sess.rs
//! Что делает: состояние одной сессии рынка KuCoin и разбор её текстовых кадров. Снимок /spotMarket/level2Depth5|50 →
//! в память и ядру (как до решения 423); у пары на быстром стакане (решение 423) снимок ядру не идёт — он запас на
//! случай сбоя и сверка с быстрым стаканом (ws_fast.rs). Изменения /market/level2 → ws_fast.rs. Неразборчивый снимок,
//! отказ подписки темы снимков → стакан пары недостоверен; отказ подписки темы изменений → пары пачки на снимках.
//! Решение 423 (как решение 109 «кадр с тем же или меньшим номером ядру не отдаётся»): номера у снимков нет — стакан,
//! у которого время биржи меньше времени уже отданного ядру стакана пары, не отдаётся (время одной шкалы: timestamp
//! снимка равен time изменений — факт живой записи, fast_check.rs). Только при быстром стакане; без него — как раньше.
//! Решение 324: стакан ядру — gateway/core_out_book.rs. Сырые кадры стакана (снимки и изменения) — в журнал сырых
//! кадров только при raw.book_frames = да (решение 109), в окно — кадры снимков, как раньше.
//! Кто вызывает: ws_conn.rs (session).
//! От чего зависит: depth.rs, book.rs, ws.rs (invalidate, End), ws_fast.rs, fast_state.rs, ctx.rs,
//! gateway/core_out_book.rs, gateway/feed_reg.rs, gateway/tap.rs.
//! Настройки: raw.book_frames [kucoin]; быстрый стакан — ws.rs (Cfg.fast).

use std::collections::HashMap;

use tokio::sync::mpsc;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;
use crate::gateway::feed_reg::ConnHandle;
use crate::gateway::tap::{TapData, TapStream};
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::ctx::{Ctx, ID};
use super::depth::{self, Msg};
use super::fast_state::Fast;
use super::ws::{End, invalidate};

/// Суффикс id подписки темы изменений пачки (id темы снимков — «sub{N}», изменений — «sub{N}f»).
pub const FAST_ID: &str = "f";
/// Начало темы изменений стакана.
pub const INCR_TOPIC: &str = "/market/level2:";

/// Сессия соединения.
pub struct Sess<'a> {
    pub ctx: &'a Ctx,
    pub name: &'a str,
    pub books: &'a Books,
    pub out: &'a mpsc::Sender<MarketEvent>,
    pub kept: BookOut,
    pub alive: Alive,
    pub feed: &'a ConnHandle,
    pub by_symbol: HashMap<&'a str, &'a Asset>,
    /// id подписки → символы пачки.
    pub by_id: HashMap<String, Vec<&'a str>>,
    /// Уровней ядру (venue.depth_levels).
    pub levels: usize,
    /// Время биржи (мс) последнего отданного ядру стакана пары.
    pub given: HashMap<String, i64>,
    pub dropped: u64,
    pub fast: Option<Fast>,
}

impl<'a> Sess<'a> {
    /// Что делает: разбор и обработка текстового кадра. Принимает: текст, время получения (нс). Возвращает: Some —
    /// сессия кончается (канал ядра закрыт). Ошибок нет.
    pub async fn on_text(&mut self, text: &str, rx_ns: i64) -> Option<End> {
        let msg = depth::parse(text);
        let is_book = match &msg {
            Msg::Depth { .. }
            | Msg::Bad {
                symbol: Some(_), ..
            } => true,
            Msg::Data { topic, .. } => topic.starts_with(INCR_TOPIC),
            _ => false,
        };
        if !is_book || matches!(self.ctx.flag("raw.book_frames"), Ok(true)) {
            self.ctx.raw.write(ID, self.name, text);
        }
        let end = match msg {
            Msg::Depth {
                symbol,
                bids,
                asks,
                exchange_time,
            } => {
                let asset = *self.by_symbol.get(symbol.as_str())?;
                self.feed.frame(&asset.ticker);
                let name = self.name;
                self.ctx
                    .taps
                    .push(ID, &asset.ticker, TapStream::Book, name, rx_ns, || {
                        TapData::Text(text.to_string())
                    });
                let book = OrderBook {
                    venue: self.ctx.venue(),
                    asset: asset.clone(),
                    bids,
                    asks,
                    exchange_time,
                    received_at: chrono::Utc::now(),
                };
                self.on_depth(&symbol, asset, book).await
            }
            Msg::Data { topic, data, .. } if topic.starts_with(INCR_TOPIC) => {
                self.on_incr(&data).await
            }
            Msg::Bad { symbol, reason } => {
                if let Some(a) = symbol.as_deref().and_then(|s| self.by_symbol.get(s)) {
                    self.feed.frame(&a.ticker);
                    invalidate(self.ctx, self.books, self.out, &a.ticker);
                }
                let m = format!("WebSocket {}: кадр не разобран: {reason}", self.name);
                self.ctx.log(Level::Error, "venue.ws", m).await;
                None
            }
            Msg::Error { id, text } => self.on_error(id, text).await,
            _ => None,
        };
        if self.dropped > 0 && self.dropped.is_power_of_two() {
            tracing::warn!(
                conn = self.name,
                dropped = self.dropped,
                "ядро не успевает принимать стаканы KuCoin"
            );
        }
        end
    }

    /// Снимок пары: у быстрой пары — запас и сверка (ws_fast.rs reconcile), иначе — ядру. Возвращает: как on_text.
    async fn on_depth(&mut self, symbol: &str, asset: &'a Asset, book: OrderBook) -> Option<End> {
        if let Some(f) = self.fast.as_mut() {
            f.snap.insert(symbol.to_string(), book.clone());
            if f.on.contains(symbol) {
                return self.reconcile(symbol, asset, &book).await;
            }
        }
        self.give(symbol, book)
    }

    /// Отказ сервера: тема снимков — стаканы пачки недостоверны; тема изменений — пары пачки на снимках. Журнал.
    /// Возвращает: как on_text.
    async fn on_error(&mut self, id: Option<String>, text: String) -> Option<End> {
        let id = id.unwrap_or_default();
        let (base, incr) = match id.strip_suffix(FAST_ID) {
            Some(b) if self.by_id.contains_key(b) => (b.to_string(), true),
            _ => (id.clone(), false),
        };
        let syms: Vec<&'a str> = self.by_id.get(&base).cloned().unwrap_or_default();
        for s in syms {
            let Some(a) = self.by_symbol.get(s).copied() else {
                continue;
            };
            if incr {
                if let Some(end) = self.fallback(s, a, "отказ подписки темы изменений").await
                {
                    return Some(end);
                }
            } else {
                invalidate(self.ctx, self.books, self.out, &a.ticker);
            }
        }
        let m = format!("WebSocket {}: ошибка KuCoin: {text}", self.name);
        self.ctx.log(Level::Error, "venue.ws", m).await;
        None
    }

    /// Что делает: стакан пары — в память и ядру, если он не старее отданного (по времени биржи; времени нет —
    /// отдаётся). Принимает: символ, стакан. Возвращает: Some(End::Done) — канал ядра закрыт. Ошибок нет.
    pub fn give(&mut self, symbol: &str, book: OrderBook) -> Option<End> {
        if !self.newer(symbol, &book) {
            return None;
        }
        if let Some(t) = book.exchange_time {
            self.given.insert(symbol.to_string(), t.timestamp_millis());
        }
        self.books.put(book.clone(), self.alive.clone());
        match self.kept.book(self.out, book) {
            Fed::Sent | Fed::Kept => None,
            Fed::Closed => Some(End::Done),
            Fed::Lost => {
                self.dropped += 1;
                None
            }
        }
    }

    /// Не старее ли стакан уже отданного ядру (по времени биржи; равное — можно). Без быстрого стакана
    /// (venue.full_book_per_sec не задана) — всегда да: снимки ядру, как до решения 423 (ревью). Ошибок нет.
    pub fn newer(&self, symbol: &str, book: &OrderBook) -> bool {
        if self.fast.is_none() {
            return true;
        }
        match (book.exchange_time, self.given.get(symbol)) {
            (Some(t), Some(g)) => t.timestamp_millis() >= *g,
            _ => true,
        }
    }
}
