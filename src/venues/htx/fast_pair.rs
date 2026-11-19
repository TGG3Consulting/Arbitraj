//! Файл: venues/htx/fast_pair.rs
//! Что делает: решение 422 — пары одного соединения быстрого канала HTX: изменения mbp.$levels → локальный стакан
//! (fast_book.rs); запросы снимка {"req"} — очередь соединения (подписка принята → запрос; разрыв, ошибка запроса,
//! неразборчивый кадр → новый запрос не раньше venue.ws_reconnect_pause_ms, урок 12; расхождение со снимком
//! mbp.refresh → сразу); стакан ядру — через выбор book_mode.rs (быстрый, пока синхронизирован; уход — запасной
//! снимок). Подписка пары не принята (у 5/20 уровней — только пары из списка документации) — пара на снимках до
//! конца сессии, запросов нет.
//! Кто вызывает: fast_conn.rs (сессия), fast_ws.rs (back — конец сессии).
//! От чего зависит: fast_book.rs, fast_reqs.rs, book_mode.rs, book.rs, book_fast.rs, fast_log.rs, mbp.rs,
//! ws.rs (invalidate), gateway/core_out_book.rs (решение 324).
//! Настройки: из fast_ws.rs (FastCfg).

use std::collections::HashMap;
use std::time::Duration;

use chrono::{TimeZone, Utc};

use tokio::sync::mpsc;

use crate::core::types::{Asset, OrderBook, PriceLevel};
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;

use super::book::{Alive, Books};
use super::book_mode::Back;
use super::ctx::Ctx;
use super::fast_book::{Applied, LocalBook, Update};
use super::fast_reqs::Reqs;
use super::fast_ws::FastCfg;
use super::ws::invalidate;

/// Состояние пар соединения на одну сессию.
pub struct Conn<'a> {
    pub ctx: &'a Ctx,
    pub books: &'a Books,
    pub out: &'a mpsc::Sender<MarketEvent>,
    pub kept: BookOut,
    pub alive: Alive,
    pub cfg: &'a FastCfg,
    pub by_symbol: HashMap<&'a str, &'a Asset>,
    pub local: HashMap<String, LocalBook>,
    /// Очередь запросов снимка (fast_reqs.rs).
    pub reqs: Reqs,
}

/// Уход пары с быстрого стакана (book_mode.rs fall_back): запасной снимок в память со своим временем кадра и ядру
/// или «стакан недостоверен». Принимает: окружение, стаканы, канал ядра, отдачу ядру, тикер, причину. Возвращает:
/// false — канал ядра закрыт. Ошибок нет.
pub fn back(
    ctx: &Ctx,
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    kept: &BookOut,
    ticker: &str,
    why: &str,
) -> bool {
    match books.mode.fall_back(ticker, why) {
        Back::Nothing => true,
        Back::Use(b, alive) => {
            books.put_at(b.clone(), alive, b.received_at.timestamp_millis());
            kept.book(out, b) != Fed::Closed
        }
        Back::Invalidate => {
            invalidate(ctx, books, out, ticker);
            true
        }
    }
}

impl<'a> Conn<'a> {
    /// Монета символа этого соединения. Ошибок нет.
    fn asset(&self, symbol: &str) -> Option<&'a Asset> {
        self.by_symbol.get(symbol).copied()
    }

    /// Кадр изменений. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub async fn on_update(&mut self, symbol: &str, u: Update) -> bool {
        if self.reqs.is_off(symbol) || self.asset(symbol).is_none() {
            return true;
        }
        let cap = self.cfg.levels as usize;
        let a = self
            .local
            .entry(symbol.to_string())
            .or_insert_with(|| LocalBook::new(cap))
            .apply(u);
        self.after(symbol, a).await
    }

    /// Ответ на запрос снимка. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub async fn on_refresh(
        &mut self,
        symbol: &str,
        seq: i64,
        bids: &[PriceLevel],
        asks: &[PriceLevel],
    ) -> bool {
        self.reqs.answered(symbol);
        if self.reqs.is_off(symbol) || self.asset(symbol).is_none() {
            return true;
        }
        let cap = self.cfg.levels as usize;
        let a = self
            .local
            .entry(symbol.to_string())
            .or_insert_with(|| LocalBook::new(cap))
            .snapshot(seq, bids, asks);
        self.after(symbol, a).await
    }

    /// Ошибка биржи по своему id: запрос снимка (REQ_ID + символ) — новый запрос после паузы; подписка (символ) —
    /// пара на снимках до конца сессии. Возвращает: false — канал ядра закрыт; id чужой — true. Ошибок нет.
    pub async fn on_error(&mut self, id: &str, text: &str) -> bool {
        if let Some(symbol) = id.strip_prefix(super::mbp::REQ_ID) {
            self.reqs.answered(symbol);
            return self
                .broken(symbol, format!("запрос снимка {symbol}: {text}"))
                .await;
        }
        let Some(asset) = self.asset(id) else {
            return true;
        };
        self.reqs.turn_off(id);
        self.local.remove(id);
        self.books.mode.log().rejected(&format!("{id}: {text}"));
        let ok = back(
            self.ctx,
            self.books,
            self.out,
            &self.kept,
            &asset.ticker,
            "подписка не принята",
        );
        self.books.mode.log().flush(self.ctx).await;
        ok
    }

    /// Стакан пары недостоверен (разрыв номеров, ошибка запроса, неразборчивый кадр, расхождение со снимком): сброс,
    /// уход на снимки, новый запрос снимка (после разрыва — через паузу). Принимает: символ, причину. Возвращает:
    /// false — канал ядра закрыт. Ошибок нет.
    pub async fn broken(&mut self, symbol: &str, why: String) -> bool {
        let Some(asset) = self.asset(symbol) else {
            return true;
        };
        if let Some(lb) = self.local.get_mut(symbol) {
            lb.reset(why.clone());
        }
        self.books.mode.log().resync(&why);
        let ok = back(
            self.ctx,
            self.books,
            self.out,
            &self.kept,
            &asset.ticker,
            &why,
        );
        self.reqs.queue(symbol, self.cfg.pause);
        self.books.mode.log().flush(self.ctx).await;
        ok
    }

    /// Расхождение со снимком (book_mode.rs Diverged — пара уже на снимках): сброс и запрос снимка сразу. Ошибок нет.
    pub async fn on_resync(&mut self, symbol: &str) {
        if self.reqs.is_off(symbol) {
            return;
        }
        if let Some(lb) = self.local.get_mut(symbol) {
            lb.reset("расхождение со снимком mbp.refresh".into());
        }
        self.books
            .mode
            .log()
            .resync(&format!("{symbol}: расхождение со снимком mbp.refresh"));
        self.reqs.queue(symbol, Duration::ZERO);
        self.books.mode.log().flush(self.ctx).await;
    }

    /// Что делать после изменения локального стакана. Возвращает: false — канал ядра закрыт. Ошибок нет.
    async fn after(&mut self, symbol: &str, a: Applied) -> bool {
        let Some(asset) = self.asset(symbol) else {
            return true;
        };
        match a {
            Applied::Changed => {
                let Some(lb) = self.local.get(symbol) else {
                    return true;
                };
                if !self.books.mode.on_fast(&asset.ticker, lb.ts()) {
                    return true;
                }
                let (bids, asks) = lb.top(self.cfg.depth);
                let book = OrderBook {
                    venue: self.ctx.venue(),
                    asset: asset.clone(),
                    bids,
                    asks,
                    exchange_time: lb.ts().and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
                    received_at: Utc::now(),
                };
                self.books.put(book.clone(), self.alive.clone());
                self.kept.book(self.out, book) != Fed::Closed
            }
            Applied::Same | Applied::Repeat => {
                if self.books.mode.fast_alive(&asset.ticker) {
                    self.books.touch(&asset.ticker);
                }
                true
            }
            Applied::Waiting => {
                // Подписка принята — запрос уже в очереди; здесь — только страховка от потерянной постановки.
                if !self.reqs.queued(symbol) {
                    self.reqs.queue(symbol, Duration::ZERO);
                }
                true
            }
            Applied::Gap(why) => self.broken(symbol, format!("{symbol}: {why}")).await,
        }
    }
}
