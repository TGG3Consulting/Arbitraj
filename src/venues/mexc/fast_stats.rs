//! Файл: venues/mexc/fast_stats.rs
//! Что делает: решение 420 — счётчики быстрого стакана MEXC на весь адаптер: переключения «быстрый → снимки» и
//! «снимки → быстрый», пересинхронизации (снимок REST после разрыва, переподписки, сбоя) и неудавшиеся снимки. Видны в
//! интерфейсе одной повторяющейся строкой журнала (решение 155: одна строка, счётчик повторов и время последнего;
//! storage/journal_counted.rs — без ожидания базы): текст строки — итог счётчиков и последнее событие (пара, причина).
//! Новых строк на каждое событие нет; события ограничены темпом снимков (venue.fast_book_snapshot_per_sec).
//! Кто вызывает: fast_pair.rs.
//! От чего зависит: storage/journal.rs, storage/journal_counted.rs, ctx.rs.
//! Настройки: нет.

use std::sync::Mutex;

use chrono::Utc;
use serde_json::json;

use crate::storage::journal::{Journal, Level};

use super::ctx::ID;

/// Вид события быстрого стакана.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ev {
    /// Ядру — снова стаканы потока снимков (страховка).
    ToSnap,
    /// Ядру — снова быстрый стакан (синхронизирован).
    ToFast,
    /// Запрошен снимок REST для пересинхронизации пары.
    Resync,
    /// Снимок REST не получен.
    SnapFail,
}

/// Счётчики и метка строки журнала.
#[derive(Default)]
struct St {
    to_snap: u64,
    to_fast: u64,
    resyncs: u64,
    snap_fail: u64,
    /// Метка повторяющейся строки; None — строки ещё нет.
    token: Option<u64>,
    repeats: i64,
}

/// Один адаптер MEXC на процесс — одни счётчики (как очередь снимков fast_snap.rs).
static STATS: Mutex<St> = Mutex::new(St {
    to_snap: 0,
    to_fast: 0,
    resyncs: 0,
    snap_fail: 0,
    token: None,
    repeats: 0,
});

/// Что делает: учитывает событие и обновляет строку журнала (синхронно, без ожидания). Принимает: журнал, событие,
/// тикер пары, причину. Ошибок нет (очередь журнала полна — строка будет создана на следующем событии).
pub fn note(journal: &Journal, ev: Ev, ticker: &str, why: &str) {
    let mut s = STATS.lock().unwrap_or_else(|p| p.into_inner());
    match ev {
        Ev::ToSnap => s.to_snap += 1,
        Ev::ToFast => s.to_fast += 1,
        Ev::Resync => s.resyncs += 1,
        Ev::SnapFail => s.snap_fail += 1,
    }
    let what = match ev {
        Ev::ToSnap => "на снимки",
        Ev::ToFast => "на быстрый",
        Ev::Resync => "пересинхронизация",
        Ev::SnapFail => "снимок REST не получен",
    };
    let msg = format!(
        "MEXC быстрый стакан (решение 420): на снимки {} раз, обратно на быстрый {}, пересинхронизаций {}, сбоев снимка {}; последнее — {ticker}: {what}: {why}",
        s.to_snap, s.to_fast, s.resyncs, s.snap_fail
    );
    let data = Some(json!({
        "to_snap": s.to_snap,
        "to_fast": s.to_fast,
        "resyncs": s.resyncs,
        "snapshot_failed": s.snap_fail,
        "ticker": ticker,
    }));
    match s.token {
        Some(t) => {
            s.repeats += 1;
            journal.update_queued(t, s.repeats, Utc::now(), msg, data);
        }
        None => {
            s.token = journal.counted_queued(Level::Warn, "venue.fast_book", Some(ID), msg, data);
            s.repeats = 1;
        }
    }
}
