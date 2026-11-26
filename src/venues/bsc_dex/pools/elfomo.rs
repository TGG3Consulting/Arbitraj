//! Файл: venues/bsc_dex/pools/elfomo.rs.
//! Что делает: проверяет ElfomoFi Base и читает котировки partnerId=0 от кошелька на одном каноническом блоке.
//! Кто вызывает: factory_ask, verify_stable_u, load, quoter. От чего зависит: ABI и снимок elfomofi_base.json.
//! Настройки: dex.pmm_pool, dex.oracle, dex.pool_implementation, dex.vault, dex.pmm_ladder_usd, dex.pmm_ladder_counter.
use super::super::{ctx::Ctx, node::rpc};
use super::{discover::Found, verify::Candidate, verify_woo};
use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, address, b256, keccak256};
use serde::Deserialize;
use serde_json::{Value, json};

mod abi;
mod market;
pub use abi::IElfomo;
pub use market::{load, quote, within_capacity};

pub(super) const ROUTER: Address = address!("f0f0f0f0fb0d738452efd03a28e8be14c76d5f73");
const PRICING: Address = address!("ffffffbb2d432b8acb4c57d556c0c721a431d038");
const VAULT: Address = address!("bb1b19f138db3925883a96ff7a304277460e0c99");
const USDC: Address = address!("833589fcd6edb6e08f4c7c32d4f71b54bda02913");
const WETH: Address = address!("4200000000000000000000000000000000000006");
const CBBTC: Address = address!("cbb7c0000ab88b473b1f5afd9ef808440eed33bf");
const IMPL: Address = address!("223987c32325ba6eafca5b38aeae13a1c80e235d");

pub fn pair(coin: Address, counter: Address) -> bool {
    counter == USDC && (coin == WETH || coin == CBBTC)
}
pub fn listed(raw: &[u8], coin: Address, counter: Address) -> Result<bool, String> {
    let pairs = IElfomo::getSupportedPairsCall::abi_decode_returns_validate(raw)
        .map_err(|e| e.to_string())?;
    Ok(pair(coin, counter)
        && pairs.iter().any(|p| {
            (p.tokenA, p.tokenB) == (coin, counter) || (p.tokenA, p.tokenB) == (counter, coin)
        }))
}

pub(super) async fn block(ctx: &Ctx, tag: &str) -> GatewayResult<Value> {
    #[derive(Deserialize)]
    struct Header {
        hash: B256,
    }
    let h: Header = rpc::call(ctx, "eth_getBlockByNumber", json!([tag, false])).await?;
    Ok(json!({"blockHash":h.hash,"requireCanonical":true}))
}

/// Проверяет фиксированные ссылки из конструктора, код роутера и текущую реализацию pricing.
pub(super) async fn verify_at(
    ctx: &Ctx,
    id: &str,
    router: Address,
    tag: &Value,
) -> GatewayResult<()> {
    let wrong = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Изменилась сеть, размещение или реализация ElfomoFi",
        )
    };
    if ctx.chain_id() != Some(8453)
        || router != ROUTER
        || ctx.dex_addr("dex.pmm_pool", id)? != ROUTER
        || ctx.dex_addr("dex.oracle", id)? != PRICING
        || ctx.dex_addr("dex.pool_implementation", id)? != IMPL
        || ctx.dex_addr("dex.vault", id)? != VAULT
    {
        return Err(wrong());
    }
    let impl_slot = super::abi_woo::implementation_slot();
    let word: B256 = rpc::call(ctx, "eth_getStorageAt", json!([PRICING, impl_slot, tag])).await?;
    if Address::from_word(word) != IMPL {
        return Err(wrong());
    }
    for (addr, expected) in [
        (
            ROUTER,
            b256!("ea30384390487d3c69cf1457cac8d768d0ec6f2cbb9e29de948e40b9a386eb59"),
        ),
        (
            PRICING,
            b256!("4d9be648c5bf39973670d9f8b481d5d0b971e6a2db2deccc6b98cde21c5dd83e"),
        ),
        (
            IMPL,
            b256!("366d1ba06b126b868f54510ecc4a3e64110d72f9e49de1e1d22e20148e041636"),
        ),
    ] {
        let code: Bytes = rpc::call(ctx, "eth_getCode", json!([addr, tag])).await?;
        if keccak256(code) != expected {
            return Err(wrong());
        }
    }
    Ok(())
}

pub async fn verify(ctx: &Ctx, c: &Candidate, router: Address) -> GatewayResult<Found> {
    let tag = block(ctx, "latest").await?;
    verify_at(ctx, c.family.id, router, &tag).await?;
    let query = Bytes::from(IElfomo::getSupportedPairsCall {}.abi_encode());
    let raw: Bytes = rpc::call(ctx, "eth_call", json!([{"to":router,"data":query},tag])).await?;
    if !listed(&raw, c.coin, c.quote).map_err(|e| ctx.err(ErrorCode::BadResponse, e))? {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Для пары ElfomoFi нет подтверждённого полного обмена",
        ));
    }
    Ok(verify_woo::found(
        c,
        PoolRef::Member {
            contract: router,
            coin: c.coin,
        },
    ))
}
