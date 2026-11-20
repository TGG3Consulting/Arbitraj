//! Файл: venues/bsc_dex/pools/load_crypto_tweak_rpc.rs
//! Что делает: решение 472 (T472-03) — пачка JSON-RPC модели tweak_price crypto v1 фабрики на блоке чтения:
//! eth_getStorageAt(пул, слот not_adjusted) и eth_getBalance(пул) (ETH монеты WETH20), ответы — словами по 32 байта
//! в конец ответов пачки вызовов. Вынесено из load_crypto_tweak_v1.rs (предел размера файла; пути — pub use там).
//! Кто вызывает: pools/load_crypto_tweak_v1.rs (реэкспорт), load_crypto_v1.rs (read), load_crypto_tweak_list.rs
//! (slot_word).
//! От чего зависит: node/rpc.rs, load_crypto_tweak_v1.rs (NOT_ADJUSTED_SLOT).
//! Настройки: нет.
//!
//! Источник: ethereum/execution-apis src/eth/state.yaml (eth_getStorageAt — слово 32 байта, eth_getBalance — число).

use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::load_crypto_tweak_v1::NOT_ADJUSTED_SLOT;

/// Параметры пачки JSON-RPC: eth_getStorageAt(пул, слот not_adjusted, блок), eth_getBalance(пул, блок). Ошибок нет.
pub fn rpc_params(pool: Address, tag: &str) -> Vec<(&'static str, Value)> {
    let slot = format!("{NOT_ADJUSTED_SLOT:#x}");
    vec![
        ("eth_getStorageAt", json!([pool, slot, tag])),
        ("eth_getBalance", json!([pool, tag])),
    ]
}

/// Слово слота и баланс пула (пачка JSON-RPC) как два ответа по 32 байта. Ошибки: узел (вся пачка); ответ одного
/// запроса не по формату — его Err в списке.
pub async fn words(
    ctx: &Ctx,
    pool: Address,
    tag: &str,
) -> GatewayResult<Vec<Result<Bytes, String>>> {
    let got = rpc::batch(ctx, &rpc_params(pool, tag)).await?;
    let mut out = Vec::with_capacity(2);
    for (k, r) in got.into_iter().enumerate() {
        out.push(match r {
            Err(e) => Err(e.message),
            Ok(v) if k == 0 => slot_word(&v),
            Ok(v) => balance_word(&v),
        });
    }
    Ok(out)
}

/// Ответ eth_getStorageAt — слово 32 байта. Ошибки (текст): не слово.
pub fn slot_word(v: &RawValue) -> Result<Bytes, String> {
    match serde_json::from_str::<B256>(v.get()) {
        Ok(w) => Ok(Bytes::copy_from_slice(w.as_slice())),
        Err(e) => Err(format!("eth_getStorageAt: {e}")),
    }
}

/// Ответ eth_getBalance (число 0x…) — слово 32 байта. Ошибки (текст): не число.
fn balance_word(v: &RawValue) -> Result<Bytes, String> {
    match serde_json::from_str::<U256>(v.get()) {
        Ok(b) => Ok(Bytes::from(b.to_be_bytes::<32>().to_vec())),
        Err(e) => Err(format!("eth_getBalance: {e}")),
    }
}
