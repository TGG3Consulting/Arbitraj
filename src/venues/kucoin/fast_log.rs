//! Файл: venues/kucoin/fast_log.rs
//! Что делает: решение 423 — счётчики быстрого стакана KuCoin (переходы быстрый ↔ снимки, пересинхронизации и их
//! сбои, расхождения со снимком) и ОДНА повторяющаяся строка журнала на процесс (решение 155, склейка: строка со
//! счётчиком повторов, без новых строк на каждое событие) — Тигран видит в интерфейсе (экран журнала), как часто это
//! бывает: текст строки — все счётчики и последнее событие, счётчик повторов — сумма событий. Запись — через очередь
//! журнала без ожидания (storage/journal_counted.rs, решение 354): одна операция очереди на событие.
//! Кто вызывает: ws_fast.rs.
//! От чего зависит: ctx.rs (журнал), storage/journal_counted.rs.
//! Настройки: нет (журнал — journal.queue_max, как у всех записей).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};

/// Событие быстрого стакана.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ev {
    /// Пара перешла на быстрый стакан (синхронизирован и отдан ядру).
    ToFast = 0,
    /// Быстрая пара перешла на снимки (разрыв номеров, расхождение, переподписка, отказ подписки).
    ToSnap = 1,
    /// Снимок REST применён (пересинхронизация удалась).
    Resync = 2,
    /// Пересинхронизация не удалась (снимок не получен или накопленные кадры его не продолжают).
    ResyncFail = 3,
    /// Кадр снимков не совпал с быстрым стаканом (fast_check.rs).
    Diverge = 4,
}

/// Счётчики по Ev (один адаптер KuCoin на процесс — состояние модуля, как пауза пулов в reply.rs).
static COUNTS: [AtomicU64; 5] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];
/// Метка повторяющейся строки журнала; None — строки ещё нет (или очередь была полна).
static ROW: Mutex<Option<u64>> = Mutex::new(None);

/// Текущие счётчики: [на быстрый, на снимки, пересинхронизаций, сбоев, расхождений]. Ошибок нет.
pub fn counts() -> [u64; 5] {
    std::array::from_fn(|i| COUNTS[i].load(Ordering::Relaxed))
}

/// Что делает: событие пары — счётчик +1 и обновление строки журнала (Warn для ToSnap, ResyncFail, Diverge; Info —
/// иначе: уровень строки — по первому событию). Принимает: окружение, событие, тикер, причину. Ошибок нет (очередь
/// журнала полна — потеря посчитана журналом, строка создастся на следующем событии).
pub fn note(ctx: &Ctx, ev: Ev, ticker: &str, why: &str) {
    COUNTS[ev as usize].fetch_add(1, Ordering::Relaxed);
    let mut row = ROW.lock().unwrap_or_else(|p| p.into_inner());
    let c = counts();
    let text = format!(
        "KuCoin, быстрый стакан (решение 423): на быстрый {} раз, на снимки {}, пересинхронизаций {} (сбоев {}), \
         расхождений со снимком {}; последнее — {ticker}: {why}",
        c[0], c[1], c[2], c[3], c[4]
    );
    let total: u64 = c.iter().sum();
    match *row {
        Some(token) => {
            ctx.journal
                .update_queued(token, total as i64, chrono::Utc::now(), text, None)
        }
        None => {
            let level = match ev {
                Ev::ToFast | Ev::Resync => Level::Info,
                _ => Level::Warn,
            };
            *row = ctx
                .journal
                .counted_queued(level, "venue.book_fast", Some(ID), text, None);
        }
    }
}
