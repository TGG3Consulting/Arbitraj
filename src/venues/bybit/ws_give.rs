//! Файл: venues/bybit/ws_give.rs
//! Что делает: решение 428 — отдача стакана Bybit ядру и в память (общая для основного потока и страховки) и действия
//! при потере потока пары: основной потерян — сразу стакан страховки (если свежий и новее отданного) или «стакан
//! недостоверен», как раньше; страховка потеряна — снять стакан пары, если он был из страховки. Всё — под замком пары
//! (backup.rs), чтобы два соединения не перемешали порядок отдачи.
//! Кто вызывает: ws_lane.rs (кадры), ws_book.rs (отказ подписки), ws.rs (обрыв сессии).
//! От чего зависит: backup.rs, backup_log.rs, book.rs, ws.rs (invalidate), gateway/core_out_book.rs, ctx.rs.
//! Настройки: venue.book_stale_ms [bybit] — стакан страховки старше этого не отдаётся (то же правило свежести, что у
//! котировки, book.rs); venue.book_backup_levels [bybit] — глубина страховки (1, 50, 200 или 1000, не равна
//! venue.depth_levels; не задана — страховки нет, как раньше).
//!
//! Официально (Orderbook → Depths → Spot): 1, 50, 200, 1000 (market.rs DEPTH_LEVELS); «Supports different depths».

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::core::types::OrderBook;
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;

use super::backup::{Held, Hub, Lost, Main};
use super::backup_log::{Ev, Stats};
use super::book::{Alive, Books};
use super::ctx::Ctx;
use super::market::DEPTH_LEVELS;
use super::ws::{Cfg, invalidate};

/// Страховка одной подписки: выбор источника по парам и счётчики. Создаёт market.rs, общая для всех соединений.
#[derive(Default)]
pub struct Insure {
    pub hub: Hub,
    pub stats: Stats,
    /// Глубина страховки orderbook.{N} (venue.book_backup_levels на момент подписки); 0 — страховки нет.
    pub levels: u64,
}

/// Общая ссылка на страховку подписки.
pub type Shared = Arc<Insure>;

/// Стакан ядру и в память. Принимает: стаканы, отдачу ядру соединения, канал ядра, стакан, живость его соединения.
/// Возвращает: Fed (Closed — канал ядра закрыт). Ошибок нет.
pub fn give(
    books: &Books,
    kept: &BookOut,
    out: &mpsc::Sender<MarketEvent>,
    book: OrderBook,
    alive: Alive,
) -> Fed {
    books.put(book.clone(), alive);
    kept.book(out, book)
}

/// Срок свежести кадра страховки — venue.book_stale_ms. Не задана — ноль (страховка не отдаётся; котировок Bybit без
/// этой настройки и так нет, book.rs). Ошибок нет.
pub fn fresh(ctx: &Ctx) -> Duration {
    Duration::from_millis(ctx.int("venue.book_stale_ms").unwrap_or(0))
}

/// Основной поток пары потерян. Принимает: окружение, стаканы, канал ядра, страховку, тикер, новое состояние основного
/// (Resync — разрыв u или расхождение; Off — обрыв соединения, отказ подписки), причину. Возвращает: Fed отдачи
/// страховки (None — не отдавалась). Ошибок нет: страховки нет или она не годится — стакан пары недостоверен (как
/// раньше, ws.rs invalidate).
pub fn main_lost(
    ctx: &Ctx,
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    ins: &Insure,
    ticker: &str,
    state: Main,
    why: &str,
) -> Option<Fed> {
    let mut fed = None;
    let fresh = fresh(ctx);
    let r = ins
        .hub
        .main_lost(ticker, state, fresh, |h: Option<&Held>| match h {
            Some(h) => {
                let kept = BookOut::new(out, ctx.venue(), &ctx.settings, &ctx.journal);
                fed = Some(give(books, &kept, out, h.book.clone(), h.alive.clone()));
            }
            None => invalidate(ctx, books, out, ticker),
        });
    if ins.hub.on() {
        let c = ins.hub.compares();
        ins.stats.hit(ctx, Ev::Resync, ticker, why, c);
        if matches!(r, Lost::Gave) {
            ins.stats.hit(ctx, Ev::ToBackup, ticker, why, c);
        }
    }
    fed
}

/// Страховка пары потеряна. Принимает: окружение, стаканы, канал ядра, страховку, тикер, причину. Ошибок нет.
pub fn backup_lost(
    ctx: &Ctx,
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    ins: &Insure,
    ticker: &str,
    why: &str,
) {
    let dropped = ins
        .hub
        .backup_lost(ticker, || invalidate(ctx, books, out, ticker));
    let tail = if dropped {
        " — стакан был из страховки, снят"
    } else {
        ""
    };
    ins.stats.hit(
        ctx,
        Ev::BackupLost,
        ticker,
        &format!("{why}{tail}"),
        ins.hub.compares(),
    );
}

/// Глубина страховки из настроек. Принимает: окружение, глубину основного (venue.depth_levels). Возвращает: None — не
/// задана (страховки нет). Ошибки: текст — значение не из Orderbook → Depths → Spot или равно глубине основного (та же
/// тема дважды в документации не описана; разные глубины — «Supports different depths»).
pub fn backup_levels(ctx: &Ctx, main: u64) -> Result<Option<u64>, String> {
    let Ok(v) = ctx.text("venue.book_backup_levels") else {
        return Ok(None);
    };
    let n: u64 = v
        .trim()
        .parse()
        .map_err(|_| format!("venue.book_backup_levels = {v}: не число"))?;
    if !DEPTH_LEVELS.contains(&n) {
        return Err(format!(
            "venue.book_backup_levels = {n}: Bybit поддерживает только {DEPTH_LEVELS:?}"
        ));
    }
    if n == main {
        return Err(format!(
            "venue.book_backup_levels = {n}: равна venue.depth_levels — нужна другая глубина"
        ));
    }
    Ok(Some(n))
}

/// Настройки соединения страховки: та же сессия, канал orderbook.{глубина страховки}, ядру — venue.depth_levels
/// уровней. Принимает: окружение, страховку подписки, настройки основного. Ошибки: текст — страховка снята или
/// глубина изменилась (новая — со следующей подпиской: сверка K считается при подписке), значение неверно.
pub fn backup_cfg(ctx: &Ctx, ins: &Insure, mut cfg: Cfg) -> Result<Cfg, String> {
    match backup_levels(ctx, cfg.levels)? {
        Some(n) if n == ins.levels => {
            cfg.levels = n;
            Ok(cfg)
        }
        Some(n) => Err(format!(
            "venue.book_backup_levels изменена ({} → {n}) — новая глубина страховки со следующей подпиской",
            ins.levels
        )),
        None => Err("venue.book_backup_levels не задана — страховка стакана выключена".into()),
    }
}
