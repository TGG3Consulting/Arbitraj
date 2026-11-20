//! Файл: venues/phemex/book_lane.rs
//! Что делает: решение 427 — поток стакана одного соединения Phemex (основной или запасной, book_pick.rs) и действия по
//! выбору: отдать стакан ядру, при потере потока — стакан запасного потока или «недостоверно»; счёт событий и запись
//! журнала `venue.book_reserve` на 1-е, 2-е, 4-е, 8-е… событие каждого вида (как решение 334 без интервала: шквал даёт
//! несколько строк, а не тысячи; в тексте — всего с запуска, чтобы видеть частоту). Читает настройку venue.book_reserve.
//! Ревью 427: без запасного потока события выбора не считаются и не пишутся (журнал как до 427: разрыв — venue.ws,
//! ws_conn.rs); запись в память и отдача ядру — под замком пары (book_pick.rs frame_with / lost_with).
//! Кто вызывает: market.rs (reserve_kind, Lane), ws.rs (обрыв соединения), ws_conn.rs (кадры, разрывы).
//! От чего зависит: book_pick.rs, book.rs, ws.rs (invalidate), ctx.rs, gateway/core_out_book.rs (решение 324).
//! Настройки: venue.book_reserve [phemex] (необязательная).
//!
//! Официально (Spot Websocket API, https://phemex-docs.github.io/#subscribe-orderbook-2): «Subscribe orderBook» —
//! params ["<symbol>"], «depth = 30 and interval = 20ms»; «Subscribe full orderBook» — params ["<symbol>", true],
//! «full depth and interval = 100ms»; «On each successful subscription, DataGW will immediately send the current …
//! snapshot». Лимиты (API Rate Limits): «concurrent connection limit to 5», «subscription limit to 20» на соединение.

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use tokio::sync::mpsc;

use crate::core::types::OrderBook;
use crate::gateway::core_out_book::BookOut;
use crate::gateway::errors::ErrorCode;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::book::{Alive, Books};
use super::book_pick::{Event, Out, Pick, Role};
use super::ctx::Ctx;
use super::ws::invalidate;

/// Код записи журнала решения 427.
const CODE: &str = "venue.book_reserve";
/// Видов событий (book_pick::Event).
const KINDS: usize = 9;

/// Счёт событий с запуска (по виду; общий для всех подписок адаптера — живёт в Books).
#[derive(Debug, Default)]
pub struct Stats {
    n: [AtomicU64; KINDS],
}

/// Поток соединения: роль, полный стакан (params [symbol, true]) и общий выбор подписки.
#[derive(Clone)]
pub struct Lane {
    pub role: Role,
    pub full: bool,
    pub pick: Pick,
}

/// Читает venue.book_reserve. Возвращает: None — не задана (запасного потока нет); Some(false) — `depth30`
/// (orderbook.subscribe [symbol]); Some(true) — `full` ([symbol, true]). Ошибка — текст: значение не из списка или
/// хранилище не читается.
pub fn reserve_kind(ctx: &Ctx) -> Result<Option<bool>, String> {
    match ctx.text("venue.book_reserve") {
        Ok(v) if v == "depth30" => Ok(Some(false)),
        Ok(v) if v == "full" => Ok(Some(true)),
        Ok(v) => Err(format!(
            "venue.book_reserve = {v}: у Phemex depth30 или full"
        )),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(e.message),
    }
}

/// Номер вида события в счёте и его текст. Ошибок нет.
fn kind(e: Event) -> (usize, Level, &'static str) {
    match e {
        Event::MainGap => (0, Level::Warn, "основной поток потерян"),
        Event::ReserveGap => (1, Level::Warn, "запасной поток потерян"),
        Event::ToReserve => (2, Level::Warn, "ядру переключён запасной поток"),
        Event::ToMain => (3, Level::Info, "основной поток снова главный (снимок)"),
        Event::MainResync => (
            4,
            Level::Info,
            "основной поток снова синхронизирован (снимок)",
        ),
        Event::ReserveResync => (
            5,
            Level::Info,
            "запасной поток снова синхронизирован (снимок)",
        ),
        Event::Blind => (
            6,
            Level::Warn,
            "оба потока не синхронизированы — стакан недостоверен до снимка",
        ),
        Event::NoTime => (7, Level::Warn, "кадр без timestamp — ядру не отдан"),
    }
}

/// Счёт контрольных снимков, разошедшихся со стаканом в памяти (local_book.rs Applied::Differs).
const SNAP_DIFF: usize = 8;

/// Счёт и, на 1-м, 2-м, 4-м… событии вида, запись журнала. Принимает: окружение, стаканы (счёт), номер и уровень
/// вида, текст, тикер, причину. Ошибок нет.
async fn hit(ctx: &Ctx, books: &Books, k: (usize, Level, &str), ticker: &str, why: &str) {
    let (i, level, text) = k;
    let total = books.lane_stats.n[i].fetch_add(1, Ordering::Relaxed) + 1;
    if total.is_power_of_two() {
        let m = format!(
            "Phemex {ticker}: {text}; {why}. Всего с запуска {total} (запись на 1-й, 2-й, 4-й… раз)"
        );
        ctx.log(level, CODE, m).await;
    }
}

/// События выбора — в счёт и журнал. Ошибок нет.
async fn note(ctx: &Ctx, books: &Books, ev: &[Event], ticker: &str, why: &str) {
    for e in ev {
        hit(ctx, books, kind(*e), ticker, why).await;
    }
}

/// Контрольный снимок разошёлся со стаканом в памяти (стакан уже заменён снимком). Ошибок нет.
pub async fn snap_differs(ctx: &Ctx, books: &Books, ticker: &str, conn: &str) {
    let k = (
        SNAP_DIFF,
        Level::Warn,
        "контрольный снимок с тем же sequence разошёлся со стаканом в памяти",
    );
    hit(
        ctx,
        books,
        k,
        ticker,
        &format!("соединение {conn}, стакан заменён снимком"),
    )
    .await;
}

impl Lane {
    /// Кадр изменил синхронизированный стакан потока. Принимает: тикер, стакан, timestamp кадра (нс), живость
    /// соединения, deliver — запись в память и отдача ядру (вызывается под замком пары, только если стакан отдаётся).
    /// Возвращает: итог deliver (None — не отдан) и события для events. Ошибок нет.
    pub fn take<R>(
        &self,
        ticker: &str,
        book: OrderBook,
        ns: Option<i64>,
        alive: &Alive,
        deliver: impl FnOnce(OrderBook) -> R,
    ) -> (Option<R>, Vec<Event>) {
        let now = Utc::now().timestamp_millis();
        self.pick
            .frame_with(self.role, ticker, book, ns, alive, now, deliver)
    }

    /// События выбора — в счёт и журнал (только при запасном потоке). Принимает: окружение, стаканы, события, тикер,
    /// причину. Ошибок нет.
    pub async fn events(&self, ctx: &Ctx, books: &Books, ev: &[Event], ticker: &str, why: &str) {
        if self.pick.reserve() && !ev.is_empty() {
            note(ctx, books, ev, ticker, why).await;
        }
    }

    /// Поток пары потерян: стакан другого потока ядру, стакан в памяти остаётся или «недостоверно» (book_pick.rs).
    /// Принимает: окружение, стаканы, канал ядра, тикер, причину. Ошибок нет.
    pub async fn lose(
        &self,
        ctx: &Ctx,
        books: &Books,
        out: &mpsc::Sender<MarketEvent>,
        ticker: &str,
        why: &str,
    ) {
        let ((), ev) = self.pick.lost_with(self.role, ticker, |act| match act {
            Out::Give(t) => {
                books.put_at(t.book.clone(), t.alive, t.seen_ms);
                // Решение 324: стакан ядру тем же путём, что кадр соединения.
                let kept = BookOut::new(out, ctx.venue(), &ctx.settings, &ctx.journal);
                let _ = kept.book(out, t.book);
            }
            Out::Keep(alive) => books.rebind(ticker, alive),
            Out::Invalidate => invalidate(ctx, books, out, ticker),
            Out::Nothing => {}
        });
        self.events(ctx, books, &ev, ticker, why).await;
    }
}
