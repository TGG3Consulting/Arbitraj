//! Файл: core/shutdown.rs
//! Что делает: решение 356 (ревью t158 Б2) — остановка процесса по Ctrl+C (и SIGTERM в Linux). По первому сигналу
//! сначала сразу режим системы «Стоп» (risk::set_mode — тот же механизм, что кнопка «Стоп» экрана «Управление»): новые
//! сделки не открываются, идущие этим не обрываются. Затем очередь уведомлений — ждём не дольше notify.stop_wait_ms
//! (не задана — не ждём), неотправленные — одна запись журнала. Затем журнал — дописываем до границы на момент этого
//! шага (Journal::flushed, новые записи не ждём). Затем выход с кодом 0. Второй такой сигнал во время ожиданий — выход
//! сразу.
//! Идущая сделка (перевод, продажа) при выходе прерывается так же, как до ветки t158 при SIGTERM: после запуска она
//! «прервана перезапуском» (ledger_load.rs), учёт согласован (записи учёта — до действия с деньгами).
//!
//! До ветки t158 обработчика не было: сигнал завершал процесс сразу (без «Стопа» и без дописывания журнала).
//! По документации tokio, после установки обработчика сигнал больше не завершает процесс сам — поэтому выход здесь
//! явный. Ставится после запуска ядра (main.rs); до этого сигнал завершает процесс сразу, как раньше.
//! Кто вызывает: main.rs (install).
//! От чего зависит: core/commands.rs (set_mode_by), core/risk.rs (Mode), notify/queue.rs (idle, pending),
//! storage/journal.rs (flushed), settings, tokio::signal.
//! Настройки: notify.stop_wait_ms (необязательная).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;
use super::risk::Mode;

/// Предел ожидания очереди уведомлений при остановке, мс.
pub const STOP_WAIT: &str = "notify.stop_wait_ms";

/// Поставлен ли обработчик (один раз за запуск).
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Ставит обработчик остановки (повторный вызов — ничего). Нет рантайма tokio — ничего. Принимает: ядро. Ошибок нет.
pub fn install(core: Core) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    rt.spawn(async move {
        stop_signal().await;
        tracing::warn!(
            "остановка: «Стоп» новых входов, дописываю журнал (повторный сигнал — выход сразу)"
        );
        tokio::select! {
            _ = Box::pin(stop(&core)) => {}
            _ = stop_signal() => {
                tracing::warn!("остановка: повторный сигнал — выход без дописывания");
            }
        }
        std::process::exit(0);
    });
}

/// Шаги остановки до выхода (см. шапку). Принимает: ядро. Ошибок нет.
async fn stop(core: &Core) {
    core.set_mode_by(Mode::Stopped, "остановка процесса (Ctrl+C / SIGTERM)")
        .await;
    let deps = &core.inner.deps;
    if let Ok(ms) = deps.settings.integer(STOP_WAIT, At::default()) {
        let _ = tokio::time::timeout(Duration::from_millis(ms), crate::notify::queue::idle()).await;
    }
    let left = crate::notify::queue::pending();
    if left > 0 {
        let text = format!(
            "Остановка: {left} уведомлений не отправлено (очередь не опустела; предел ожидания — {STOP_WAIT})"
        );
        deps.log(
            Level::Warn,
            crate::notify::queue::LOST_KIND,
            None,
            text,
            None,
        )
        .await;
    }
    deps.journal.flushed().await;
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
