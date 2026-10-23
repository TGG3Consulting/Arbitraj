//! Файл: venues/bsc_dex/wallet/nonce.rs
//! Что делает: строгая очередь транзакций кошелька бота (решение 45): в каждый момент в работе только одна
//! транзакция — от подписи до момента, когда её nonce занят в блоке (её самой, отменой или чужой транзакцией).
//! Очередь не освобождается раньше: ни ошибка чтения квитанции, ни «не попала», ни отмена вызывающего (сделка
//! прервана) — место держит задача трекера (exec/txs.rs), пока nonce не занят.
//! Кто вызывает: exec/trade.rs, exec/trade_smart.rs, wallet/transfer.rs (место в очереди), exec/txs.rs (nonce, удержание).
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
    /// Наименьший nonce, который может взять следующая транзакция (прошлые заняты в блоке). None — не было отправок.
    next: Arc<StdMutex<Option<u64>>>,
}

/// Место в очереди: пока живо оно или хотя бы одно удержание (`hold`), другие транзакции кошелька ждут.
pub struct Slot {
    guard: Arc<OwnedMutexGuard<()>>,
    next: Arc<StdMutex<Option<u64>>>,
    /// Адрес кошелька.
    pub from: Address,
}

/// Удержание очереди задачей трекера: очередь свободна, только когда удержаний и самого места не осталось.
#[derive(Clone)]
pub struct Hold {
    _guard: Arc<OwnedMutexGuard<()>>,
    next: Arc<StdMutex<Option<u64>>>,
}

impl Nonces {
    /// Встаёт в очередь (ждёт, пока прошлая транзакция кошелька займёт свой nonce в блоке).
    /// Возвращает: Slot. Ошибки: ключ кошелька (SecretsLocked, NotConfigured).
    pub async fn acquire(&self, ctx: &Ctx) -> GatewayResult<Slot> {
        let guard = self.lock.clone().lock_owned().await;
        let from = signer::address(ctx)?;
        Ok(Slot {
            guard: Arc::new(guard),
            next: self.next.clone(),
            from,
        })
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
        }
    }
}

impl Hold {
    /// Nonce занят в блоке: следующий — не меньше `next`. Ошибок нет.
    pub fn consumed(&self, next: u64) {
        let mut n = self.next.lock().unwrap_or_else(|p| p.into_inner());
        *n = Some(n.map_or(next, |k| k.max(next)));
    }
}
