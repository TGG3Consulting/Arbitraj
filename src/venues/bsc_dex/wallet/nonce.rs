//! Файл: venues/bsc_dex/wallet/nonce.rs
//! Что делает: строгая очередь транзакций кошелька бота (решение 45): в каждый момент в работе только одна
//! транзакция — от подписи до момента, когда её nonce занят в блоке (её самой, отменой или чужой транзакцией).
//! Очередь не освобождается раньше: ни ошибка чтения квитанции, ни «не попала», ни отмена вызывающего (сделка
//! прервана) — место держит задача трекера (exec/txs.rs), пока nonce не занят.
//! Исход не установлен (лимиты отмены, время ожидания send.queue_max_wait_s, nonce не перечитан) — очередь
//! «остановлена до ручной проверки»: новые транзакции не берутся, пока человек не снимет её (release, кнопка
//! «Снять очередь после ручной проверки»). Состояние — WalletQueueState (экран, уведомления).
//! Кто вызывает: exec/trade.rs, exec/trade_smart.rs, wallet/transfer.rs (место в очереди), exec/txs.rs (nonce, удержание),
//! mod.rs (wallet_queue, release_wallet_queue).
//! От чего зависит: ctx.rs, node/rpc.rs, wallet/signer.rs.
//! Настройки: узел — bsc.node_http_url.
//!
//! Почему так:
//! - 48 Club просит не слать с одного адреса параллельно; nonce идут строго по порядку (решение 45).
//! - «The sender is responsible for signing the transaction and using the correct nonce» —
//!   https://github.com/bnb-chain/bsc/blob/947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7/internal/ethapi/api.go#L2412-L2433
//! - Nonce = eth_getTransactionCount(адрес, "latest") — число транзакций кошелька в блоках
//!   (https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/state.yaml).
//!   Следующая транзакция берёт nonce только после того, как прошлый занят в блоке (send/track.rs), поэтому
//!   «latest» всегда верен; если узел HTTP отстал и ещё не видит прошлый nonce занятым — берётся запомненный
//!   «следующий» (`next`), он не меньше.

use std::sync::{Arc, Mutex as StdMutex};

use alloy_primitives::{Address, B256, U64};
use chrono::Utc;
use serde_json::json;
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::core::types::WalletQueueState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::{queue_store, signer};

/// Очередь кошелька: одна на адаптер. Дешёво клонируется.
#[derive(Clone)]
pub struct Nonces {
    lock: Arc<Mutex<()>>,
    /// Наименьший nonce, который может взять следующая транзакция (прошлые заняты в блоке). None — не было отправок.
    next: Arc<StdMutex<Option<u64>>>,
    /// Состояние очереди (свободна / ждёт итога / остановлена до ручной проверки).
    state: Arc<StdMutex<WalletQueueState>>,
    /// Прочитано ли состояние из SQLite после запуска (wallet/queue_store.rs).
    restored: Arc<Mutex<bool>>,
}

impl Default for Nonces {
    fn default() -> Self {
        Self {
            lock: Arc::default(),
            next: Arc::default(),
            state: Arc::new(StdMutex::new(WalletQueueState::Free)),
            restored: Arc::default(),
        }
    }
}

/// Место в очереди: пока живо оно или хотя бы одно удержание (`hold`), другие транзакции кошелька ждут.
pub struct Slot {
    guard: Arc<OwnedMutexGuard<()>>,
    next: Arc<StdMutex<Option<u64>>>,
    state: Arc<StdMutex<WalletQueueState>>,
    /// Адрес кошелька.
    pub from: Address,
}

/// Удержание очереди задачей трекера: очередь свободна, только когда удержаний и самого места не осталось.
#[derive(Clone)]
pub struct Hold {
    _guard: Arc<OwnedMutexGuard<()>>,
    next: Arc<StdMutex<Option<u64>>>,
    state: Arc<StdMutex<WalletQueueState>>,
}

impl Nonces {
    /// Встаёт в очередь (ждёт, пока прошлая транзакция кошелька займёт свой nonce в блоке).
    /// Возвращает: Slot. Ошибки: OutcomeUnknown — очередь остановлена до ручной проверки (причина в тексте);
    /// ключ кошелька (SecretsLocked, NotConfigured).
    pub async fn acquire(&self, ctx: &Ctx) -> GatewayResult<Slot> {
        let guard = self.lock.clone().lock_owned().await;
        self.restore(ctx).await;
        if let WalletQueueState::Halted { reason, .. } = self.state() {
            return Err(ctx.err(
                ErrorCode::OutcomeUnknown,
                format!("Очередь кошелька остановлена до ручной проверки: {reason}"),
            ));
        }
        let from = signer::address(ctx)?;
        Ok(Slot {
            guard: Arc::new(guard),
            next: self.next.clone(),
            state: self.state.clone(),
            from,
        })
    }

    /// Один раз после запуска читает состояние из SQLite (wallet/queue_store.rs); не прочитано — очередь
    /// остановлена до ручной проверки (лучше стоять, чем отправить второй раз). Ошибок нет.
    pub async fn restore(&self, ctx: &Ctx) {
        let mut done = self.restored.lock().await;
        if *done {
            return;
        }
        *done = true;
        let st = match queue_store::load(ctx).await {
            Ok(Some(s)) => s,
            Ok(None) => return,
            Err(e) => WalletQueueState::Halted {
                reason: format!("состояние очереди после запуска не прочитано из SQLite: {e}"),
                nonce: None,
                hash: None,
                since: Utc::now(),
            },
        };
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = st;
    }

    /// Состояние очереди. Ошибок нет.
    pub fn state(&self) -> WalletQueueState {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// «Снять очередь после ручной проверки»: остановленная очередь снова свободна, запомненный nonce забыт (берётся
    /// у узла). Возвращает: новое состояние. Ошибки: Err(причина) — очередь не остановлена.
    pub fn release(&self) -> Result<WalletQueueState, String> {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if !matches!(*st, WalletQueueState::Halted { .. }) {
            return Err("очередь кошелька не остановлена — снимать нечего".into());
        }
        *st = WalletQueueState::Free;
        *self.next.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(st.clone())
    }

    /// Занята ли очередь сейчас (для интерфейса и журнала). Ошибок нет.
    pub fn busy(&self) -> bool {
        self.lock.try_lock().is_err()
    }
}

impl Slot {
    /// Nonce для следующей транзакции: eth_getTransactionCount(кошелёк, "latest"), но не меньше запомненного.
    /// Ошибки: узел (связь, ответ).
    pub async fn nonce(&self, ctx: &Ctx) -> GatewayResult<u64> {
        let n: U64 =
            rpc::call(ctx, "eth_getTransactionCount", json!([self.from, "latest"])).await?;
        let known = *self.next.lock().unwrap_or_else(|p| p.into_inner());
        Ok(known.map_or(n.to(), |k| k.max(n.to())))
    }

    /// Удержание очереди для задачи трекера. Ошибок нет.
    pub fn hold(&self) -> Hold {
        Hold {
            _guard: self.guard.clone(),
            next: self.next.clone(),
            state: self.state.clone(),
        }
    }
}

impl Hold {
    /// Очередь ждёт итога транзакции. Возвращает: новое состояние. Ошибок нет.
    pub fn busy(&self, nonce: u64, hash: B256) -> WalletQueueState {
        self.set(WalletQueueState::Busy {
            nonce,
            hash,
            since: Utc::now(),
        })
    }

    /// Итог установлен — очередь свободна. Возвращает: новое состояние. Ошибок нет.
    pub fn free(&self) -> WalletQueueState {
        self.set(WalletQueueState::Free)
    }

    /// Исход не установлен — очередь остановлена до ручной проверки. Возвращает: новое состояние. Ошибок нет.
    pub fn halt(&self, reason: String, nonce: u64, hash: B256) -> WalletQueueState {
        self.set(WalletQueueState::Halted {
            reason,
            nonce: Some(nonce),
            hash: Some(hash),
            since: Utc::now(),
        })
    }

    /// Ставит состояние. Ошибок нет.
    fn set(&self, s: WalletQueueState) -> WalletQueueState {
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = s.clone();
        s
    }

    /// Nonce занят в блоке: следующий — не меньше `next`. Ошибок нет.
    pub fn consumed(&self, next: u64) {
        let mut n = self.next.lock().unwrap_or_else(|p| p.into_inner());
        *n = Some(n.map_or(next, |k| k.max(next)));
    }
}
