//! Файл: venues/bsc_dex/wallet/allow_cache.rs
//! Что делает: разрешения кошелька (ERC-20 → Permit2 и Permit2 → Universal Router) в памяти для котировки (решение 61):
//! котировка живого экрана идёт на каждом тике, и два eth_call разрешений на каждую (quote/approve_gas.rs) забивали
//! очередь узла. Разрешения кошелька меняются только его собственными транзакциями (approve, обмен списывает
//! разрешение) — поэтому значения держатся до любой транзакции кошелька: при отправке и при её итоге память
//! очищается (exec/txs.rs), следующее чтение — с узла. Сделка и проба читают разрешения с узла всегда (plan.rs).
//! Кто вызывает: quote/approve_gas.rs (read), exec/txs.rs (clear).
//! От чего зависит: wallet/approve.rs (Read, read).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use alloy_primitives::Address;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::approve::{self, Read};

/// Ключ: (токен, Permit2, Universal Router).
type Key = (Address, Address, Address);

/// Разрешения по (токен, Permit2, Universal Router). Дешёво клонируется; одна на адаптер (parts.rs).
#[derive(Clone, Default)]
pub struct Allowances {
    map: Arc<Mutex<HashMap<Key, Read>>>,
}

impl Allowances {
    /// Разрешения из памяти или с узла (и в память). Принимает: токен, кошелёк, Permit2, Universal Router.
    /// Ошибки: узел.
    pub async fn read(
        &self,
        ctx: &Ctx,
        token: Address,
        wallet: Address,
        permit2: Address,
        ur: Address,
    ) -> GatewayResult<Read> {
        let key = (token, permit2, ur);
        if let Some(r) = self.map.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return Ok(*r);
        }
        let r = approve::read(ctx, token, wallet, permit2, ur).await?;
        self.map
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, r);
        Ok(r)
    }

    /// Транзакция кошелька ушла или завершилась — разрешения могли измениться. Ошибок нет.
    pub fn clear(&self) {
        self.map.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}
