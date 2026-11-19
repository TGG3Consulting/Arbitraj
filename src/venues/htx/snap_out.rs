//! Файл: venues/htx/snap_out.rs
//! Что делает: решение 422 — кадр снимков mbp.refresh (соединение /ws) при быстром стакане: без быстрой ветки — как
//! до решения 422 (в память и ядру; тот же seqNum — только время кадра пары); с ней — выбор book_mode.rs: пара на
//! снимках — снимок ядру (не старее отданного), пара на быстром — снимок только запасной и сверка по содержимому;
//! расхождение — уход на снимки (fast_pair.rs back) и пересинхронизация быстрого стакана.
//! Кто вызывает: ws_conn.rs.
//! От чего зависит: book.rs, book_mode.rs, fast_pair.rs (back), fast_log.rs, ctx.rs, gateway/core_out_book.rs.
//! Настройки: venue.book_stale_ms [htx] (только при несовпадении снимка с быстрым стаканом).

use tokio::sync::mpsc;

use crate::core::types::OrderBook;
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;

use super::book::{Alive, Books};
use super::book_mode::Snap;
use super::ctx::Ctx;
use super::fast_pair::back;

/// Куда отдавать: окружение, стаканы, канал ядра, отдача ядру, живость соединения снимков, venue.depth_levels.
pub struct To<'a> {
    pub ctx: &'a Ctx,
    pub books: &'a Books,
    pub out: &'a mpsc::Sender<MarketEvent>,
    pub kept: &'a BookOut,
    pub alive: &'a Alive,
    pub levels: usize,
}

impl To<'_> {
    /// Новый снимок пары. Принимает: стакан. Возвращает: Fed (Sent — и когда ядру ничего не отдавалось). Ошибок нет.
    pub async fn give(&self, book: OrderBook) -> Fed {
        if !self.books.mode.enabled() {
            self.books.put(book.clone(), self.alive.clone());
            return self.kept.book(self.out, book);
        }
        let ticker = book.asset.ticker.clone();
        match self.pick(&ticker, Some(&book)) {
            Snap::Give => {
                self.books.put(book.clone(), self.alive.clone());
                self.kept.book(self.out, book)
            }
            Snap::Touch => {
                self.books.touch(&ticker);
                Fed::Sent
            }
            Snap::Hold => Fed::Sent,
            Snap::Diverged => self.diverged(&ticker).await,
        }
    }

    /// Снимок с тем же seqNum (решение 109). Принимает: тикер. Возвращает: false — канал ядра закрыт. Ошибок нет.
    pub async fn repeat(&self, ticker: &str) -> bool {
        if !self.books.mode.enabled() {
            self.books.touch(ticker);
            return true;
        }
        match self.pick(ticker, None) {
            Snap::Touch => self.books.touch(ticker),
            Snap::Diverged => return self.diverged(ticker).await != Fed::Closed,
            Snap::Give | Snap::Hold => {}
        }
        true
    }

    /// Выбор book_mode.rs. Ошибок нет (venue.book_stale_ms не задана — расхождения по времени нет).
    fn pick(&self, ticker: &str, frame: Option<&OrderBook>) -> Snap {
        let stale = || self.ctx.int("venue.book_stale_ms").ok().map(|v| v as i64);
        self.books
            .mode
            .on_snapshot(self.books, ticker, frame, self.alive, self.levels, stale)
    }

    /// Расхождение: пара на снимки, просьба пересинхронизации, запись журнала (если пора). Возвращает: Fed.
    async fn diverged(&self, ticker: &str) -> Fed {
        let why = "снимок mbp.refresh не совпал с быстрым стаканом, а быстрый не подтверждён дольше venue.book_stale_ms";
        let open = back(self.ctx, self.books, self.out, self.kept, ticker, why);
        self.books.mode.resyncs().send(ticker);
        self.books.mode.log().flush(self.ctx).await;
        if open { Fed::Sent } else { Fed::Closed }
    }
}
