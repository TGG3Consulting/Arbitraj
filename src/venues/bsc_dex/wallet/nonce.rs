//! Файл: venues/bsc_dex/wallet/nonce.rs
//! Что делает: строгая очередь транзакций кошелька бота (решение 45): в каждый момент в работе только одна
//! транзакция — от подписи до итога (в блоке / не попала). Nonce берётся у узла при входе в очередь.
//! Кто вызывает: exec/*, wallet/approve.rs, wallet/transfer.rs (каждая отправка — через Slot).
//! От чего зависит: ctx.rs, node/rpc.rs, wallet/signer.rs.
//! Настройки: узел — bsc.node_http_url.
//!
//! Почему так:
//! - 48 Club просит не слать с одного адреса параллельно; nonce идут строго по порядку (решение 45).
//! - «The sender is responsible for signing the transaction and using the correct nonce» —
//!   https://github.com/bnb-chain/bsc/blob/947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7/internal/ethapi/api.go#L2412-L2433
//! - Nonce = eth_getTransactionCount(адрес, "latest") — число транзакций кошелька в блоках
//!   (https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/state.yaml).
//!   «latest», а не «pending»: если прошлая транзакция не попала в блок за отведённое время, новая занимает
//!   её nonce и не встаёт в очередь за ней. Если прошлая всё-таки попадёт в блок — узел или канал ответит
//!   «nonce too low» → NonceConflict, а трекер покажет «вытеснена» (research/flow/bsc_send.md, раздел 5).

use std::sync::Arc;

use alloy_primitives::{Address, U64};
use serde_json::json;
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::signer;

/// Очередь кошелька: одна на адаптер. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Nonces {
    lock: Arc<Mutex<()>>,
}

/// Место в очереди: пока оно живо, другие транзакции кошелька ждут. Освобождается при удалении.
pub struct Slot {
    _guard: OwnedMutexGuard<()>,
    /// Адрес кошелька.
    pub from: Address,
    /// Nonce для этой транзакции.
    pub nonce: u64,
}

impl Nonces {
    /// Встаёт в очередь (ждёт, пока прошлая транзакция кошелька закончится) и берёт nonce у узла.
    /// Возвращает: Slot. Ошибки: ключ кошелька (SecretsLocked, NotConfigured), узел (связь, ответ).
    pub async fn acquire(&self, ctx: &Ctx) -> GatewayResult<Slot> {
        let guard = self.lock.clone().lock_owned().await;
        let from = signer::address(ctx)?;
        let n: U64 = rpc::call(ctx, "eth_getTransactionCount", json!([from, "latest"])).await?;
        Ok(Slot {
            _guard: guard,
            from,
            nonce: n.to(),
        })
    }

    /// Занята ли очередь сейчас (для интерфейса и журнала). Ошибок нет.
    pub fn busy(&self) -> bool {
        self.lock.try_lock().is_err()
    }
}
