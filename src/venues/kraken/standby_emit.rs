//! Файл: venues/kraken/standby_emit.rs
//! Что делает: решение 426 — связка соединения стаканов Kraken с выбором стакана (standby.rs): роль соединения и общий
//! выбор пар (Link); сверенный кадр — offer, потеря стакана пары — lose. Отдача — стакан в память адаптера (book.rs, с
//! живостью соединения-источника и временем его кадра — правила свежести котировки те же) и ядру (решение 324,
//! gateway/core_out_book.rs); «недостоверен» — ws.rs invalidate. Без страховки (venue.book_standby_depth не задана —
//! выбора нет) — как раньше: каждый сверенный кадр отдаётся, потеря — «недостоверен». Каждое переключение и каждая
//! потеря — счётчик и запись журнала venue.book_standby (standby_log.rs).
//! Кто вызывает: ws_conn.rs (кадр, разрыв, отказ подписки), ws.rs (обрыв соединения — все пары).
//! От чего зависит: standby.rs, standby_log.rs, book.rs, ws.rs (invalidate), ctx.rs, gateway/core_out_book.rs.
//! Настройки: log.pool_events_lost_interval_s (интервал записи журнала; необязательная).

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::Books;
use super::ctx::Ctx;
use super::standby::{Arbiter, Frame, Role};
use super::standby_log::Event;
use super::ws::invalidate;

/// Роль соединения и выбор стакана его пар (None — страховки нет, всё как раньше).
#[derive(Clone)]
pub struct Link {
    pub role: Role,
    pub arb: Option<Arc<Arbiter>>,
}

/// Куда отдавать: окружение, стаканы адаптера, канал ядра, отдача стаканов ядру.
pub struct Out<'a> {
    pub ctx: &'a Ctx,
    pub books: &'a Books,
    pub out: &'a mpsc::Sender<MarketEvent>,
    pub kept: &'a BookOut,
}

impl Out<'_> {
    /// Стакан кадра — в память адаптера и ядру. Возвращает: Fed. Ошибок нет.
    fn put(&self, f: Frame) -> Fed {
        self.books.put_at(f.book.clone(), f.alive, f.seen_ms);
        self.kept.book(self.out, f.book)
    }

    /// Счётчик события и запись журнала, если пора (standby_log.rs). Ошибок нет.
    async fn note(&self, e: Event) {
        let every = || self.ctx.int("log.pool_events_lost_interval_s").ok();
        if let Some(m) = self.books.standby.note(e, every) {
            self.ctx.log(Level::Warn, "venue.book_standby", m).await;
        }
    }

    /// Что делает: сверенный кадр пары соединения link — ядру, если выбор его пропускает (без страховки — всегда).
    /// Принимает: связку, тикер, кадр. Возвращает: Fed отдачи; None — кадр не отдан (держится как страховка или
    /// старше отданного). Ошибок нет.
    pub async fn offer(&self, link: &Link, ticker: &str, f: Frame) -> Option<Fed> {
        let Some(arb) = &link.arb else {
            return Some(self.put(f));
        };
        let (fed, sw) = arb.offer(link.role, ticker, f, |f| self.put(f));
        if let Some(sw) = sw {
            self.note(Event::Switched(sw)).await;
        }
        fed
    }

    /// Что делает: соединение link потеряло стакан пары (разрыв чек-суммы, изменение до снимка, отказ подписки, кадр
    /// не разобран, обрыв). С выбором — стакан другой стороны или «недостоверен» (standby.rs lost); без — «недостоверен».
    /// Принимает: связку, тикер. Возвращает: Fed отдачи стакана страховки (None — не отдавался). Ошибок нет.
    pub async fn lose(&self, link: &Link, ticker: &str) -> Option<Fed> {
        let Some(arb) = &link.arb else {
            invalidate(self.ctx, self.books, self.out, ticker);
            return None;
        };
        let blind = || invalidate(self.ctx, self.books, self.out, ticker);
        let (fed, sw, lost) = arb.lost(link.role, ticker, |f| self.put(f), blind);
        self.note(Event::Lost(link.role)).await;
        if let Some(sw) = sw {
            self.note(Event::Switched(sw)).await;
        }
        if lost {
            self.note(Event::Blind).await;
        }
        fed
    }
}
