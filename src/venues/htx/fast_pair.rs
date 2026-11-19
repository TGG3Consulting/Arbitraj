//! Файл: venues/htx/fast_pair.rs
//! Что делает: решение 422 — пары одного соединения быстрого канала HTX: изменения mbp.$levels → локальный стакан
//! (fast_book.rs); запросы снимка {"req"} — очередь соединения (подписка принята → запрос; разрыв, ошибка запроса,
//! неразборчивый кадр → новый запрос не раньше venue.ws_reconnect_pause_ms, урок 12; расхождение со снимком
//! mbp.refresh → сразу); стакан ядру — через выбор book_mode.rs (быстрый, пока синхронизирован; уход — запасной
//! снимок). Подписка пары не принята (у 5/20 уровней — только пары из списка документации) — пара на снимках до
//! конца сессии, запросов нет.
//! Кто вызывает: fast_conn.rs (сессия).
//! От чего зависит: fast_book.rs, fast_reqs.rs, book_mode.rs, book.rs, book_fast.rs, fast_log.rs, mbp.rs,
//! snap_out.rs (back — уход на снимки), gateway/core_out_book.rs (решение 324).
//! Настройки: из fast_ws.rs (FastCfg).

use std::collections::HashMap;
use std::time::Duration;

use chrono::{TimeZone, Utc};

use tokio::sync::mpsc;

use crate::core::types::{Asset, OrderBook, PriceLevel};
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;

use super::book::{Alive, Books};
use super::ctx::Ctx;
use super::fast_book::{Applied, LocalBook, Update};
use super::fast_reqs::Reqs;
use super::fast_ws::FastCfg;
use super::snap_out::back;

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
        if self.reqs.is_off(symbol) {
            return true;
        }
        let Some(asset) = self.asset(symbol) else {
            return true;
        };
        // Новый снимок req: объявленное снимками расхождение снято (решение 422, ревью).
        self.books.mode.resynced(&asset.ticker);
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

    /// Расхождение со снимком (book_mode.rs Diverged — пара уже на снимках): сброс, уход на снимки ещё раз (если
    /// между расхождением и этой просьбой пара успела вернуться на быстрый по снимку req) и запрос снимка сразу.
    /// Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub async fn on_resync(&mut self, symbol: &str) -> bool {
        let Some(asset) = self.asset(symbol) else {
            return true;
        };
        if self.reqs.is_off(symbol) {
            return true;
        }
        let why = format!("{symbol}: расхождение со снимком mbp.refresh");
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
        self.reqs.queue(symbol, Duration::ZERO);
        self.books.mode.log().flush(self.ctx).await;
        ok
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
                let t = lb.ts();
                let (bids, asks) = lb.top(self.cfg.depth);
                let book = OrderBook {
                    venue: self.ctx.venue(),
                    asset: asset.clone(),
                    bids,
                    asks,
                    exchange_time: t.and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
                    received_at: Utc::now(),
                };
                // Решение «отдавать ли» и отдача — под одним замком book_mode.rs (решение 422, ревью).
                let (books, alive, kept, out) = (self.books, &self.alive, &self.kept, self.out);
                books
                    .mode
                    .on_fast(&asset.ticker, t, || {
                        books.put(book.clone(), alive.clone());
                        kept.book(out, book) != Fed::Closed
                    })
                    .unwrap_or(true)
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
