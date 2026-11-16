//! Файл: storage/journal_stop.rs
//! Что делает: решение 341 (по решению владельца — всегда) — корректная остановка бота дописывает очереди: на Ctrl+C
//! (и SIGTERM в Linux) ждёт, пока опустеет очередь уведомлений (notify/queue.rs — её задания сами пишут журнал), затем
//! очередь записи журнала, и завершает процесс с кодом 0. Второй такой сигнал во время дописывания — выход сразу (код
//! 0; недописанное теряется). Ставится один раз при запуске (main.rs). По документации tokio, после установки
//! обработчика сигнал больше не завершает процесс сам — поэтому завершение здесь явное. Запись учёта ядра
//! (storage/ledger_db.rs) очереди нет: её ждёт сам шаг сделки.
//! Кто вызывает: main.rs (install).
//! От чего зависит: storage/journal.rs (idle), notify/queue.rs (idle), tokio::signal.
//! Настройки: нет.

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
        tracing::warn!(
            "остановка: дописываю очереди уведомлений и журнала (повторный сигнал — выход сразу)"
        );
        let drain = async {
            crate::notify::queue::idle().await;
            journal.idle().await;
        };
        tokio::select! {
            _ = drain => {}
            _ = stop_signal() => {
                tracing::warn!("остановка: повторный сигнал — выход без дописывания очередей");
            }
        }
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
