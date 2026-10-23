//! Файл: core/queue_notify.rs
//! Что делает: очередь транзакций кошелька DEX в ядре — состояние в живой поток интерфейса (CoreEvent::WalletQueue →
//! /api/ws) и уведомления только на смену состояния: «остановлена до ручной проверки» (send.queue_halted) и снятие
//! остановленной (send.queue_released). Обычное «свободна» после транзакции и «ждёт итога» — без уведомлений.
//! Кто вызывает: core/tick.rs (событие адаптера WalletQueue).
//! От чего зависит: engine.rs, view.rs, deps.rs (notify).
//! Настройки: notify.event.* (send.queue_halted, send.queue_released).

use super::engine::Core;
use super::view::CoreEvent;

/// Очередь кошелька DEX: в живой поток интерфейса — всегда; уведомление — только на смену состояния:
/// стала «остановлена до ручной проверки» (send.queue_halted) или остановленную сняли (send.queue_released).
/// Обычное «свободна» после транзакции и «ждёт итога» — без уведомления. Ошибок нет.
pub async fn wallet_queue(
    core: &Core,
    q: crate::core::types::WalletQueue,
    prev: Option<crate::core::types::WalletQueueState>,
) {
    use crate::core::types::WalletQueueState as S;
    core.inner.live.send(CoreEvent::WalletQueue(q.clone()));
    let was_halted = matches!(prev, Some(S::Halted { .. }));
    let venue = q.venue.0.clone();
    let (code, text) = match &q.state {
        S::Halted { reason, .. } if !was_halted => (
            "send.queue_halted",
            format!("{venue}: отправки остановлены до ручной проверки — {reason}"),
        ),
        S::Free if was_halted => (
            "send.queue_released",
            format!("{venue}: очередь кошелька снята после ручной проверки"),
        ),
        _ => return,
    };
    core.inner.deps.notify(code, Some(&venue), &text).await;
}
