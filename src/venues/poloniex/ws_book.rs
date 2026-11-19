//! Файл: venues/poloniex/ws_book.rs
//! Что делает: решение 424 — стаканы одного соединения Poloniex из двух каналов: book_lv2 (быстрый, локальный стакан)
//! и book (снимки, страховка). Выбор стакана — fast_book.rs; здесь — отдача ядру и в память, отметки свежести сторожа
//! (решения 202/263), переподписка book_lv2 после разрыва (не чаще venue.ws_reconnect_pause_ms на пару, урок 12),
//! сводка переключений в журнал venue.book_fast не чаще log.pool_events_lost_interval_s (gateway/log_clock.rs).
//! Решение 324: стакан ядру — gateway/core_out_book.rs (venue.book_events_keep).
//! Кто вызывает: ws_conn.rs.
//! От чего зависит: ws_book_sync.rs (переподписка, сводка), fast_book.rs, lv2.rs, depth.rs, book.rs, ws.rs (invalidate), ctx.rs, gateway/{core_out_book,
//! feed_reg, log_clock}.rs.
//! Настройки: venue.book_stale_ms (срок страховочного снимка — тот же, что у котировки, book.rs);
//! venue.ws_reconnect_pause_ms (через ws.rs Cfg; не задана — переподписки book_lv2 нет, пара остаётся на снимках);
//! log.pool_events_lost_interval_s (сводка; не задана — записей нет, счёт копится).
//!
//! Официально: «If id of the last message does not match lastId of the current message» — «must re-subscribe to the
//! channel» (Market Data → Book Level 2, https://api-docs.poloniex.com/spot/websocket/market-data); отписка — Spot →
//! WebSocket, «Unsubscribe». Обе команды — в общую очередь отправки соединения (темп venue.ws_subs_per_sec, в счёт
//! «500 requests per second»), REST не нужен.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::core::types::{Asset, OrderBook};
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;
use crate::gateway::feed_reg::ConnHandle;
use crate::gateway::log_clock::Clock;

use super::book::{Alive, Books};
use super::ctx::Ctx;
use super::depth::{self, Depth};
use super::fast_book::{Act, FastBooks};
use super::lv2::Lv2;
use super::ws::invalidate;

/// Стаканы соединения.
pub struct BookConn<'a> {
    pub(super) ctx: &'a Ctx,
    pub(super) name: &'a str,
    pub(super) books: &'a Books,
    pub(super) out: &'a mpsc::Sender<MarketEvent>,
    pub(super) feed: &'a ConnHandle,
    pub(super) by_symbol: HashMap<&'a str, &'a Asset>,
    pub alive: Alive,
    pub(super) depth: usize,
    pub(super) resync_pause: Option<Duration>,
    pub(super) fast: FastBooks,
    /// Канал book: последний id по символу (решения 109, 263).
    pub(super) last_seq: HashMap<String, i64>,
    /// Пара → когда можно снова переподписать book_lv2; отложенные переподписки.
    pub(super) not_before: HashMap<String, Instant>,
    pub(super) deferred: HashMap<String, Instant>,
    pub(super) clock: Clock,
    pub(super) kept: BookOut,
    pub(super) dropped: u64,
}

impl<'a> BookConn<'a> {
    /// Новое состояние. Принимает: окружение, имя, пары (монета, символ), память, канал ядра, учёт свежести, уровней
    /// ядру (venue.depth_levels), паузу переподписки. Ошибок нет.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &'a Ctx,
        name: &'a str,
        assets: &'a [(Asset, String)],
        books: &'a Books,
        out: &'a mpsc::Sender<MarketEvent>,
        feed: &'a ConnHandle,
        depth: u64,
        resync_pause: Option<Duration>,
    ) -> Self {
        Self {
            ctx,
            name,
            books,
            out,
            feed,
            by_symbol: assets.iter().map(|(a, s)| (s.as_str(), a)).collect(),
            alive: Alive::default(),
            depth: depth as usize,
            resync_pause,
            fast: FastBooks::default(),
            last_seq: HashMap::new(),
            not_before: HashMap::new(),
            deferred: HashMap::new(),
            clock: Clock::default(),
            kept: BookOut::new(out, ctx.venue(), &ctx.settings, &ctx.journal),
            dropped: 0,
        }
    }

    /// Снимки book кадра. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub fn on_books(&mut self, list: Vec<Depth>) -> bool {
        for d in list {
            if !self.by_symbol.contains_key(d.symbol.as_str())
                || !depth::newer(&mut self.last_seq, &d)
            {
                continue;
            }
            let symbol = d.symbol.clone();
            let act = self.fast.on_book(d, self.depth);
            if !self.apply(&symbol, act) {
                return false;
            }
        }
        true
    }

    /// Записи book_lv2 кадра; разрыв — переподписка в очередь pending. Возвращает: false — канал ядра закрыт.
    pub fn on_lv2(&mut self, list: Vec<Lv2>, pending: &mut Vec<String>) -> bool {
        for d in list {
            if !self.by_symbol.contains_key(d.symbol.as_str()) {
                continue;
            }
            let ctx = self.ctx;
            let (act, why) = self.fast.on_lv2(&d, self.depth, || stale(ctx));
            if why.is_some() {
                self.resync(&d.symbol, pending);
                self.note();
            }
            if !self.apply(&d.symbol, act) {
                return false;
            }
        }
        true
    }

    /// Неразборчивый кадр пары: lv2 — разрыв (страховка и переподписка), book — страховки нет (быстрый тоже не
    /// синхронизирован — стакан пары недостоверен, как до решения 424). Возвращает: false — канал ядра закрыт.
    pub fn on_bad(
        &mut self,
        symbol: &str,
        is_lv2: bool,
        why: &str,
        pending: &mut Vec<String>,
    ) -> bool {
        let Some(asset) = self.by_symbol.get(symbol).copied() else {
            return true;
        };
        self.feed.frame(&asset.ticker);
        if is_lv2 {
            let ctx = self.ctx;
            let (act, _) = self.fast.on_bad_lv2(symbol, why.to_string(), || stale(ctx));
            self.resync(symbol, pending);
            return self.apply(symbol, act);
        }
        if self.fast.on_bad_book(symbol) {
            invalidate(self.ctx, self.books, self.out, &asset.ticker);
        }
        true
    }

    /// Переподписка пары сторожем ядра (оба канала): прежние id и стаканы пары забыты. Ошибок нет.
    pub fn forget(&mut self, symbol: &str) {
        self.fast.forget(symbol);
        self.last_seq.remove(symbol);
        self.deferred.remove(symbol);
    }

    /// Выполняет решение fast_book.rs. Возвращает: false — канал ядра закрыт. Ошибок нет.
    fn apply(&mut self, symbol: &str, act: Act) -> bool {
        let Some(asset) = self.by_symbol.get(symbol).copied() else {
            return true;
        };
        let (bids, asks, time, send) = match act {
            Act::Nothing => return true,
            Act::Invalidate => {
                invalidate(self.ctx, self.books, self.out, &asset.ticker);
                return true;
            }
            Act::Publish { bids, asks, time } => (bids, asks, time, true),
            Act::Confirm { bids, asks, time } => (bids, asks, time, false),
        };
        self.feed.frame(&asset.ticker);
        let book = OrderBook {
            venue: self.ctx.venue(),
            asset: asset.clone(),
            bids,
            asks,
            exchange_time: time,
            received_at: chrono::Utc::now(),
        };
        if !send {
            self.books.put(book, self.alive.clone());
            return true;
        }
        self.books.put(book.clone(), self.alive.clone());
        match self.kept.book(self.out, book) {
            Fed::Sent | Fed::Kept => {}
            Fed::Closed => return false,
            Fed::Lost => self.dropped += 1,
        }
        if self.dropped > 0 && self.dropped.is_power_of_two() {
            tracing::warn!(
                conn = self.name,
                dropped = self.dropped,
                "ядро не успевает принимать стаканы Poloniex"
            );
        }
        true
    }
}

/// Срок страховочного снимка — venue.book_stale_ms (не задана — страховки нет). Читается только при разрыве. Ошибок нет.
fn stale(ctx: &Ctx) -> Option<Duration> {
    ctx.int("venue.book_stale_ms")
        .ok()
        .map(Duration::from_millis)
}
