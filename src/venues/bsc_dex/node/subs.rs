//! Файл: venues/bsc_dex/node/subs.rs
//! Что делает: фильтры подписки logs пачками: по адресам контрактов (V2, V3 — у каждого пула свой адрес) или по
//! PoolId в topics[1] у менеджера-синглтона (V4). Размер пачки — dex.pools_per_sub.
//! Кто вызывает: pools/market.rs.
//! От чего зависит: ctx.rs.
//! Настройки: dex.pools_per_sub [bsc_dex].
//!
//! Фильтр logs = фильтр eth_getLogs: address — адрес или массив; topics — позиция = И, массив = ИЛИ —
//! https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml#L1-L60 ,
//! https://geth.ethereum.org/docs/interacting-with-geth/rpc/pubsub ; у узла BSC до 1000 вариантов в позиции topics
//! (eth/filters/api.go#L62-L69), лимит адресов — LogQueryLimit узла (research/flow/bsc_dex.md, раздел 4).

use alloy_primitives::{Address, B256};
use serde_json::{Value, json};

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;

/// Размер пачки из настроек. Ошибки: NotConfigured; ноль — NotConfigured с объяснением.
pub fn per_sub(ctx: &Ctx) -> GatewayResult<usize> {
    let n = ctx.int("dex.pools_per_sub")?;
    if n == 0 {
        return Err(ctx.not_set(
            "dex.pools_per_sub",
            "dex.pools_per_sub должен быть больше нуля",
        ));
    }
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

/// Фильтр по адресам пулов (V2, V3). Принимает: адреса пачки, topic0 событий. Ошибок нет.
pub fn by_address(addresses: &[Address], topic0: &[B256]) -> Value {
    json!({ "address": addresses, "topics": [topic0] })
}

/// Фильтр по PoolId у менеджера (V4). Принимает: менеджер, PoolId пачки, topic0 событий. Ошибок нет.
pub fn by_pool_id(manager: Address, ids: &[B256], topic0: &[B256]) -> Value {
    json!({ "address": manager, "topics": [topic0, ids] })
}
