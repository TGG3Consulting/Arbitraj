//! Файл: venues/okx/failover.rs
//! Что делает: решение 429 — действия страховки стакана OKX (состояние — backup.rs): кадр стакана любого соединения →
//! в память и ядру, если этот источник сейчас отдаётся; потеря синхронизации быстрого стакана (разрыв prevSeqId,
//! обрыв соединения, отказ подписки, неразборчивый кадр, переподписка сторожем) → сразу последний снимок books5 из
//! памяти (если он не старше venue.book_stale_ms и не старее отданного), иначе пара недостоверна, как раньше; потеря
//! пары страховочным соединением, пока ядро считало по её снимку, → пара недостоверна. Каждое переключение считается;
//! запись журнала venue.book_backup («потерь синхронизации, переключений на снимки, без годного снимка, возвратов на
//! быстрый, обрывов страховки» с прошлой записи) — не чаще раза в venue.book_backup_log_s (первая — сразу; часы —
//! gateway/log_clock.rs, как у venue.book_events_lost); не задана — записей нет, счёт копится.
//! Кто вызывает: ws_conn.rs (кадры и потери в сессии), ws.rs (обрыв соединения — все пары).
//! От чего зависит: backup.rs, book.rs, ws.rs (invalidate), ctx.rs, gateway/core_out_book.rs (стакан ядру),
//! storage/journal.rs.
//! Ревью 429: решение «отдавать ли» и отдача (память book.rs + канал ядра) — под замком Backup::order, общим для
//! основного и страховочного соединений: ядро получает кадры пары в том же порядке, в каком приняты решения.
//! Настройки: venue.book_stale_ms [okx] (срок свежести снимка при переключении), venue.book_backup_log_s [okx].

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;

use crate::core::types::OrderBook;
use crate::gateway::core_out_book::{BookOut, Fed};
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::backup::{Lost, Role};
use super::book::{Alive, Books};
use super::ctx::Ctx;
use super::ws::invalidate;

/// Интервал записи журнала страховки, секунды.
pub const LOG_EVERY: &str = "venue.book_backup_log_s";

/// Сейчас, unix мс.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Кадр стакана соединения (уже собранный стакан пары). Принимает: окружение, стаканы, канал ядра, отдачу ядру
/// сессии, роль соединения, живость соединения, стакан. Возвращает: None — ядру не отдан (другой источник сейчас
/// главный или кадр старее отданного); Some(Fed) — как ушёл. Ошибок нет.
pub fn deliver(
    ctx: &Ctx,
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    kept: &BookOut,
    role: Role,
    alive: &Alive,
    book: OrderBook,
) -> Option<Fed> {
    // Ревью 429: решение и отдача — под одним замком (не старее отданного и при гонке двух соединений).
    let _order = books.backup.order();
    let book = match role {
        Role::Main => {
            let t = book.exchange_time.map(|t| t.timestamp_millis());
            let (serve, back) = books.backup.fast(&book.asset.ticker, t);
            if back {
                note(ctx, books);
            }
            if !serve {
                return None;
            }
            book
        }
        Role::Backup => books.backup.snap(book, alive.clone(), now_ms())?,
    };
    books.put(book.clone(), alive.clone());
    Some(kept.book(out, book))
}

/// Пара соединения потеряна. Основное соединение: снимок books5 из памяти ядру (Lost::Serve), ядро уже на снимке —
/// ничего, годного снимка нет — пара недостоверна, если drop (переподписка сторожем передаёт false: стакан в памяти,
/// как раньше, судит свежесть book.rs). Страховочное: пара недостоверна, только если ядро считало по её снимку.
/// Принимает: окружение, стаканы, канал ядра, роль, тикер, drop. Ошибок нет (без ожидания).
pub fn lost(
    ctx: &Ctx,
    books: &Books,
    out: &mpsc::Sender<MarketEvent>,
    role: Role,
    ticker: &str,
    drop: bool,
) {
    // Ревью 429: как в deliver — снимок из памяти или «недостоверно» уходит ядру до кадра другого соединения.
    let _order = books.backup.order();
    match role {
        Role::Main => {
            // Решение 238: venue.book_stale_ms обязательна; не задана — снимок не отдаётся (как и котировка).
            let max_age = ctx.int("venue.book_stale_ms").ok().map(|v| v as i64);
            match books.backup.fast_lost(ticker, max_age, now_ms()) {
                Lost::Serve(s) => {
                    books.put_at(s.book.clone(), s.alive, s.seen_ms);
                    let kept = BookOut::new(out, ctx.venue(), &ctx.settings, &ctx.journal);
                    let _ = kept.book(out, s.book);
                }
                Lost::Keep => return,
                Lost::Drop if drop => invalidate(ctx, books, out, ticker),
                Lost::Drop => {}
            }
        }
        Role::Backup => {
            if !books.backup.snap_lost(ticker) {
                return;
            }
            invalidate(ctx, books, out, ticker);
        }
    }
    note(ctx, books);
}

/// Запись журнала venue.book_backup, если пора (зовётся только на событиях страховки; счёт обнуляется).
/// Принимает: окружение, стаканы. Ошибок нет; нет рантайма tokio — записи нет.
pub fn note(ctx: &Ctx, books: &Books) {
    let c = &books.backup.counts;
    let due = books
        .backup
        .clock
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .due(Instant::now(), || {
            ctx.int(LOG_EVERY).ok().map(Duration::from_secs)
        });
    let Some(since) = due else { return };
    let take = |a: &std::sync::atomic::AtomicU64| a.swap(0, Ordering::Relaxed);
    let (lost, to_snap, no_snap) = (take(&c.lost), take(&c.to_snap), take(&c.no_snap));
    let (to_fast, snap_gone) = (take(&c.to_fast), take(&c.snap_gone));
    let window = since.map_or("с запуска".to_string(), |d| {
        format!("за {} с", d.as_secs())
    });
    let msg = format!(
        "стакан OKX, страховка books5 (решение 429): быстрый books терял синхронизацию {lost}, переключений на снимки \
         {to_snap}, без годного снимка (пара недостоверна) {no_snap}, возвратов на быстрый {to_fast}, обрывов страховки \
         во время подмены {snap_gone} {window}"
    );
    let data = json!({
        "lost": lost, "to_snap": to_snap, "no_snap": no_snap, "to_fast": to_fast, "snap_gone": snap_gone,
        "since_s": since.map(|d| d.as_secs()),
    });
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let (journal, venue) = (ctx.journal.clone(), ctx.venue().0);
    rt.spawn(async move {
        let _ = journal
            .write(
                Level::Info,
                "venue.book_backup",
                Some(&venue),
                msg,
                Some(data),
            )
            .await;
    });
}
