//! Файл: venues/bybit/backup_log.rs
//! Что делает: решение 428 — счётчики страховки стакана Bybit (переключения основной → страховка и обратно,
//! пересинхронизации основного, расхождения со страховкой, потери страховки) и запись журнала `venue.book_backup` о
//! них. Без новой нагрузки: запись — на 1-е, 2-е, 4-е, 8-е… событие каждого вида (то же правило «степени двойки», что у
//! gateway/lost_log.rs без интервала), в тексте — все счётчики с начала подписки и число состоявшихся сверок; запись
//! — отдельной задачей tokio (кадр не ждёт). Журнал виден в интерфейсе (экран «Журнал»).
//! Кто вызывает: ws_book.rs, ws_give.rs.
//! От чего зависит: ctx.rs (журнал), storage/journal.rs, tokio.
//! Настройки: нет.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};

/// Вид события страховки.
#[derive(Clone, Copy, Debug)]
pub enum Ev {
    /// Стакан ядра — из страховки (основной потерян или расходится).
    ToBackup,
    /// Стакан ядра снова из основного после снимка.
    ToMain,
    /// Основной поток пары пересинхронизируется (разрыв u, обрыв, отказ подписки).
    Resync,
    /// Сверка при равном seq: первые K уровней разошлись.
    Mismatch,
    /// Страховка пары потеряна (её разрыв, обрыв соединения, отказ подписки).
    BackupLost,
}

/// Названия видов для журнала (порядок — как в Ev).
const NAMES: [&str; 5] = [
    "на страховку",
    "обратно на основной",
    "пересинхронизаций основного",
    "расхождений со страховкой",
    "потерь страховки",
];

/// Счётчики одной подписки. Default — нули.
#[derive(Debug, Default)]
pub struct Stats {
    n: [AtomicU64; 5],
}

impl Stats {
    /// Значения счётчиков по порядку Ev. Ошибок нет.
    pub fn all(&self) -> [u64; 5] {
        std::array::from_fn(|i| self.n[i].load(Ordering::Relaxed))
    }

    /// Одно событие: счёт и, если это 1-е, 2-е, 4-е… событие вида, — запись журнала. Принимает: окружение, вид, тикер,
    /// причину, число сверок (Hub::compares). Возвращает: счёт вида с начала подписки. Ошибок нет; нет рантайма tokio —
    /// только счёт.
    pub fn hit(&self, ctx: &Ctx, ev: Ev, ticker: &str, why: &str, compares: u64) -> u64 {
        let n = self.n[ev as usize].fetch_add(1, Ordering::Relaxed) + 1;
        if !n.is_power_of_two() {
            return n;
        }
        let all = self.all();
        let counts: Vec<String> = NAMES
            .iter()
            .zip(all)
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
        let msg = format!(
            "Страховка стакана (решение 428): {ticker} — {}: {why}. С начала подписки: {}; сверок при равном seq {compares} \
             (запись на 1-е, 2-е, 4-е… событие вида)",
            NAMES[ev as usize],
            counts.join(", ")
        );
        let data = json!({
            "ticker": ticker, "event": format!("{ev:?}"), "to_backup": all[0], "to_main": all[1], "resync": all[2],
            "mismatch": all[3], "backup_lost": all[4], "compares": compares,
        });
        let level = match ev {
            Ev::ToMain => Level::Info,
            _ => Level::Warn,
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return n;
        };
        let journal = ctx.journal.clone();
        rt.spawn(async move {
            let _ = journal
                .write(level, "venue.book_backup", Some(ID), msg, Some(data))
                .await;
        });
        n
    }
}
