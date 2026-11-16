//! Файл: storage/journal_stop.rs
//! Что делает: решение 341 — остановка бота с включённой очередью журнала (journal.queue_max) дописывает очередь: на
//! Ctrl+C (и SIGTERM в Linux) ждёт, пока очередь записи журнала опустеет, и завершает процесс с кодом 0. Обработчик
//! ставится один раз, при первом включении очереди; без настройки не ставится — остановка как раньше (по документации
//! tokio, после установки обработчика сигнал больше не завершает процесс сам — поэтому завершение здесь явное).
//! Кто вызывает: storage/write_modes.rs (install).
//! От чего зависит: storage/journal.rs (idle), tokio::signal.
//! Настройки: journal.queue_max (включает установку).

use std::sync::atomic::{AtomicBool, Ordering};

use super::journal::Journal;

/// Поставлен ли обработчик (один раз за запуск).
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Ставит обработчик остановки (повторный вызов — ничего). Нет рантайма tokio — ничего. Принимает: журнал. Ошибок нет.
pub fn install(journal: Journal) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    rt.spawn(async move {
        stop_signal().await;
        tracing::warn!("остановка: дописываю очередь журнала (journal.queue_max)");
        journal.idle().await;
        std::process::exit(0);
    });
}

/// Ждёт сигнала остановки: Ctrl+C; в Linux ещё SIGTERM (обработчик SIGTERM не поставился — только Ctrl+C; Ctrl+C не
/// поставился — ждёт SIGTERM или никогда). Ошибок нет.
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                r = tokio::signal::ctrl_c() => {
                    if r.is_err() {
                        term.recv().await;
                    }
                }
                _ = term.recv() => {}
            }
            return;
        }
    }
    // Обработчик Ctrl+C не поставился — не завершаться сразу: ждать нечего, процесс останавливается как раньше.
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}
