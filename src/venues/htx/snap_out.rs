//! Файл: venues/htx/snap_out.rs
//! Что делает: решение 422 — кадр снимков mbp.refresh (соединение /ws) при быстром стакане: без быстрой ветки — как
//! до решения 422 (в память и ядру; тот же seqNum — только время кадра пары); с ней — выбор book_mode.rs: пара на
//! снимках — снимок ядру (не старее отданного), пара на быстром — снимок только запасной и сверка по содержимому;
//! расхождение (два снимка подряд новее быстрого и не равны ему, book_rule.rs) — уход на снимки (deliver
//! ниже) и пересинхронизация быстрого стакана. Отдача — под замком book_mode.rs (решение 422, ревью).
//! Решение 422 (ревью): здесь же back / deliver — уход пары с быстрого стакана (из fast_pair.rs, ориентир 200 строк).
//! Кто вызывает: ws_conn.rs; back — fast_pair.rs, fast_ws.rs.
//! От чего зависит: book.rs, book_mode.rs, ws.rs (invalidate), fast_log.rs, ctx.rs, gateway/core_out_book.rs.
//! Настройки: нет своих.

use tokio::sync::mpsc;

use crate::core::types::OrderBook;
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;

use super::book::{Alive, Books};
use super::book_mode::{Back, Snap};
use super::ctx::Ctx;
use super::ws::invalidate;

/// Куда отдавать: окружение, стаканы, канал ядра, отдача ядру, живость соединения снимков, venue.depth_levels.
pub struct To<'a> {
    pub ctx: &'a Ctx,
    pub books: &'a Books,
    pub out: &'a mpsc::Sender<MarketEvent>,
    pub kept: &'a BookOut,
    pub alive: &'a Alive,
    pub levels: usize,
}

/// Уход пары с быстрого стакана (book_mode.rs fall_back): запасной снимок в память со своим временем кадра и ядру
/// или «стакан недостоверен» — под замком выбора (решение 422, ревью). Принимает: окружение, стаканы, канал ядра,
/// отдачу ядру, тикер, причину. Возвращает: false — канал ядра закрыт. Ошибок нет.
pub fn back(
    ctx: &Ctx,
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    kept: &BookOut,
    ticker: &str,
    why: &str,
) -> bool {
    books
        .mode
        .fall_back(ticker, why, |b| deliver(ctx, books, out, kept, ticker, b))
}

/// Отдача ухода с быстрого стакана (вызывается под замком book_mode.rs; без ожиданий). Принимает: окружение, стаканы,
/// канал ядра, отдачу ядру, тикер, Back. Возвращает: false — канал ядра закрыт. Ошибок нет.
pub fn deliver(
    ctx: &Ctx,
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    kept: &BookOut,
    ticker: &str,
    back: Back,
) -> bool {
    match back {
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

impl To<'_> {
    /// Новый снимок пары. Принимает: стакан. Возвращает: Fed (Sent — и когда ядру ничего не отдавалось). Ошибок нет.
    pub async fn give(&self, book: OrderBook) -> Fed {
        if !self.books.mode.enabled() {
            self.books.put(book.clone(), self.alive.clone());
            return self.kept.book(self.out, book);
        }
        let ticker = book.asset.ticker.clone();
        self.pick(&ticker, Some(&book)).await
    }

    /// Снимок с тем же seqNum (решение 109). Принимает: тикер. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub async fn repeat(&self, ticker: &str) -> bool {
        if !self.books.mode.enabled() {
            self.books.touch(ticker);
            return true;
        }
        self.pick(ticker, None).await != Fed::Closed
    }

    /// Выбор book_mode.rs и отдача под его замком (решение 422, ревью: без гонки с соединением /feed); расхождение —
    /// просьба пересинхронизации и запись журнала (если пора). Принимает: тикер, новый снимок (None — тот же seqNum).
    /// Возвращает: Fed. Ошибок нет.
    async fn pick(&self, ticker: &str, frame: Option<&OrderBook>) -> Fed {
        let (snap, fed) = self.books.mode.on_snapshot(
            self.books,
            ticker,
            frame,
            self.alive,
            self.levels,
            |snap, back| match (snap, frame) {
                (Snap::Give, Some(b)) => {
                    self.books.put(b.clone(), self.alive.clone());
                    self.kept.book(self.out, b.clone())
                }
                (Snap::Touch, _) => {
                    self.books.touch(ticker);
                    Fed::Sent
                }
                (Snap::Diverged, _) => {
                    if deliver(self.ctx, self.books, self.out, self.kept, ticker, back) {
                        Fed::Sent
                    } else {
                        Fed::Closed
                    }
                }
                _ => Fed::Sent,
            },
        );
        if snap == Snap::Diverged {
            self.books.mode.resyncs().send(ticker);
            self.books.mode.log().flush(self.ctx).await;
        }
        fed
    }
}
