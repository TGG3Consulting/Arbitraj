//! Проверяет ссылки и код трёх обновляемых контрактов BaiBai в каноническом состоянии Base.
//! Используют поиск, загрузка и квотер. Настройки: dex.pmm_pool, dex.oracle, dex.vault, dex.pool_implementation.
use super::super::{ctx::Ctx, node::rpc};
use super::{
    abi_woo, discover::Found, elfomo, lunar, pmm_view::view, verify::Candidate, verify_woo,
};
use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy_primitives::{Address, B256, Bytes, address, b256, keccak256};
use serde_json::{Value, json};

mod market;
pub use market::{load, quote};

pub const ENTRY: Address = address!("98c1d9e102eb2806d902b13186bdc7892ac4ffba");
const BOOK: Address = address!("604d9b9eb1e1571c78661a6c1088427ec9c8c6e5");
const VAULT: Address = address!("aac48feb93c5c97e0fb3c7c57e1633922a4acda3");
const ENTRY_IMPL: Address = address!("cc61f0acb73eea94c160af15da4471b9b456f3b0");
const PROXY_HASH: B256 = b256!("4a36b37c1e3367a597a7e97c82b6870a6b7b73884a180ffc14e486b17f9a4245");

sol! {
    interface IBai {
        struct Side { int16 spreadBps; uint16 depthBps; uint256 filled; uint8 knotCount; }
        struct Pair { uint64 seq; uint64 fillSeq; uint64 lastUpdateAt; uint80 mid; uint256 qUnit; Side ask; Side bid; }
        function quoteToken() external view returns (address);
        function curveBook() external view returns (address);
        function custodian() external view returns (address);
        function entrypoint() external view returns (address);
        function quoteFor(address base,address tokenIn,uint256 amountIn,address taker) external view returns (uint256 amountOut,uint256 fee);
        function pair(address base) external view returns (Pair memory);
        function balanceOf(address account) external view returns (uint256);
        event Fill(address indexed base,uint64 indexed fillSeq,address indexed taker,address tokenIn,uint256 amountIn,uint256 amountOut,uint256 fee);
    }
}

async fn identity(
    ctx: &Ctx,
    tag: &Value,
    proxy: Address,
    imp: Address,
    hash: B256,
) -> GatewayResult<bool> {
    let (active, proxy_code, impl_code): (B256, Bytes, Bytes) = tokio::try_join!(
        rpc::call(
            ctx,
            "eth_getStorageAt",
            json!([proxy, abi_woo::implementation_slot(), tag])
        ),
        rpc::call(ctx, "eth_getCode", json!([proxy, tag])),
        rpc::call(ctx, "eth_getCode", json!([imp, tag])),
    )?;
    Ok(Address::from_word(active) == imp
        && keccak256(proxy_code) == PROXY_HASH
        && keccak256(impl_code) == hash)
}

async fn verify_at(ctx: &Ctx, id: &str, tag: &Value) -> GatewayResult<()> {
    let wrong = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Изменилось размещение, связи или код BaiBai",
        )
    };
    if ctx.chain_id() != Some(8453)
        || ctx.dex_addr("dex.pmm_pool", id)? != ENTRY
        || ctx.dex_addr("dex.oracle", id)? != BOOK
        || ctx.dex_addr("dex.vault", id)? != VAULT
        || ctx.dex_addr("dex.pool_implementation", id)? != ENTRY_IMPL
    {
        return Err(wrong());
    }
    let zero = Address::ZERO;
    let (entry, book, vault, eb, ev, eq, be, bq, ve, vb) = tokio::try_join!(
        identity(
            ctx,
            tag,
            ENTRY,
            ENTRY_IMPL,
            b256!("1292147a441b4ad7c8c67aaeb97982096e9d5c932715ac102670c924ab4d698d")
        ),
        identity(
            ctx,
            tag,
            BOOK,
            address!("3e6abc41e9f41d7f6f7f26a6bf8851434fa69f1d"),
            b256!("b1e5561b5f7f96f74142020fa5ee43ade3f6484f08c678d93eff103641d014a7")
        ),
        identity(
            ctx,
            tag,
            VAULT,
            address!("997b2d1b539dfccc9d42f2d10fe804f0ac9b3076"),
            b256!("f824799c1dba49382fb6da460837c833572d0eaa73d4db0f7df3daf0ab5de107")
        ),
        view(ctx, tag, zero, ENTRY, IBai::curveBookCall {}),
        view(ctx, tag, zero, ENTRY, IBai::custodianCall {}),
        view(ctx, tag, zero, ENTRY, IBai::quoteTokenCall {}),
        view(ctx, tag, zero, BOOK, IBai::entrypointCall {}),
        view(ctx, tag, zero, BOOK, IBai::quoteTokenCall {}),
        view(ctx, tag, zero, VAULT, IBai::entrypointCall {}),
        view(ctx, tag, zero, VAULT, IBai::curveBookCall {}),
    )?;
    if !entry
        || !book
        || !vault
        || (eb, ev, eq, be, bq, ve, vb)
            != (BOOK, VAULT, lunar::USDC, ENTRY, lunar::USDC, ENTRY, BOOK)
    {
        return Err(wrong());
    }
    Ok(())
}

pub async fn verify(ctx: &Ctx, c: &Candidate, address: Address) -> GatewayResult<Found> {
    if address != ENTRY || !lunar::pair(c.coin, c.quote) {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Нет архивной приёмки этой пары BaiBai",
        ));
    }
    let tag = elfomo::block(ctx, "latest").await?;
    verify_at(ctx, c.family.id, &tag).await?;
    Ok(verify_woo::found(c, PoolRef::Contract(ENTRY)))
}
