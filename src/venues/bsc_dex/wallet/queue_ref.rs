//! Файл: venues/bsc_dex/wallet/queue_ref.rs
//! Что делает: решение 408 (ответ Тиграна 9б) — ссылка на состояние очереди кошелька без её удержания: для самоснятия
//! остановленной очереди по квитанции (wallet/auto_release.rs) — состояние, снять только «эту» остановку (release_if),
//! замок очереди на время снятия и записи (ревью t193 М6). Вынесено из auto_release.rs — размер файла.
//! Кто вызывает: wallet/nonce.rs (Nonces::queue_ref, Hold::queue_ref), wallet/auto_release.rs.
//! От чего зависит: core/types (WalletQueueState), tokio (Mutex).
//! Настройки: нет.

use std::sync::{Arc, Mutex};

use alloy_primitives::B256;
use tokio::sync::Mutex as AsyncMutex;

use crate::core::types::WalletQueueState;

/// Решение 408 (9б): состояние очереди без удержания (очередь не держит; замок — только на снятие, locked) — для
/// самоснятия по квитанции (release_if). Дешёво клонируется.
#[derive(Clone)]
pub struct QueueRef {
    lock: Arc<AsyncMutex<()>>,
    next: Arc<Mutex<Option<u64>>>,
    state: Arc<Mutex<WalletQueueState>>,
}

impl QueueRef {
    /// Ссылка на состояние очереди (wallet/nonce.rs Nonces::queue_ref, Hold::queue_ref). Ошибок нет.
    pub(super) fn new(
        lock: Arc<AsyncMutex<()>>,
        next: Arc<Mutex<Option<u64>>>,
        state: Arc<Mutex<WalletQueueState>>,
    ) -> Self {
        Self { lock, next, state }
    }

    /// Ревью t193 (М6): замок очереди — на время «снять и записать свободной» (новая транзакция не встанет между ними).
    /// Ошибок нет.
    pub async fn locked(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.lock.clone().lock_owned().await
    }

    /// Состояние очереди. Ошибок нет.
    pub fn state(&self) -> WalletQueueState {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Снять остановку, только если стоит именно эта транзакция; запомненный nonce забыт (как Nonces::release).
    /// Принимает: хеш. Возвращает: новое состояние или None — остановка другая или очереди уже не остановлена.
    pub fn release_if(&self, hash: B256) -> Option<WalletQueueState> {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if !matches!(&*st, WalletQueueState::Halted { hash: Some(h), .. } if *h == hash) {
            return None;
        }
        *st = WalletQueueState::Free;
        *self.next.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Some(st.clone())
    }
}
