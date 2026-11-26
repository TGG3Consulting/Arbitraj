//! Файл: venues/bsc_dex/pools/lunar.rs.
//! Что делает: сверяет активный proxy LunarBase, реализацию, порядок native ETH/USDC и код общего модуля.
//! Кто вызывает: поиск, загрузка, квотер, подготовка сделки. От чего зависит: ABI и архив lunarbase.json.
//! Настройки: dex.kind, dex.pmm_pool, dex.pool_implementation, dex.router и dex.pmm_ladder_usd.
use super::super::{ctx::Ctx, node::rpc};
use super::{discover::Found, verify::Candidate, verify_woo};
use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy_primitives::{Address, B256, Bytes, address, b256, keccak256};
use serde_json::{Value, json};

use super::pmm_view::view;
mod market;
pub use market::{load, quote};

sol! {
    interface ILunar {
        function X() external view returns (address);
        function Y() external view returns (address);
        function paused() external view returns (bool);
        function isFresh() external view returns (bool);
        function getXReserve() external view returns (uint112);
        function getYReserve() external view returns (uint112);
        function state() external view returns (uint160 anchorPrice,uint24 feeAskX24,uint24 feeBidX24,uint48 latestUpdateBlock);
        function quoteExactIn(address tokenIn,address tokenOut,uint256 amountIn) external view returns (uint256);
        function quoteLunar(address tokenIn,address tokenOut,uint256 amountIn) external view returns (uint256);
        event SwapExecuted(address recipient,bool xToY,uint256 dx,uint256 dy,uint256 fee);
        event Upgraded(address indexed implementation);
        event StateUpdated(uint160 anchor,uint24 ask,uint24 bid);
        event Sync(uint128 x,uint128 y);
        event Paused(address caller);
        event Unpaused(address caller);
        event WhitelistSet(address indexed account,bool enabled);
        event PartnerFeeSet(address indexed partner,uint32 fee);
        event BlacklistFeeMultiplierSet(uint256 multiplier);
        event BlockDelaySet(uint48 delay);
        event MaxPunishmentX24Set(uint24 value);
    }
}
pub const POOL: Address = address!("0000efc4ec03a7c47d3a38a9be7ff1d52dd01b99");
const IMPL: Address = address!("88460bd8dae6f632b9124e99b09d584e0b34b80f");
pub const WETH: Address = address!("4200000000000000000000000000000000000006");
pub const USDC: Address = address!("833589fcd6edb6e08f4c7c32d4f71b54bda02913");

pub fn pair(coin: Address, counter: Address) -> bool {
    coin == WETH && counter == USDC
}

pub(super) async fn open(ctx: &Ctx, tag: &Value, owner: Address) -> GatewayResult<bool> {
    let (paused, fresh) = tokio::try_join!(
        view(ctx, tag, owner, POOL, ILunar::pausedCall {}),
        view(ctx, tag, owner, POOL, ILunar::isFreshCall {})
    )?;
    Ok(!paused && fresh)
}

pub(super) async fn verify_at(ctx: &Ctx, id: &str, tag: &Value) -> GatewayResult<()> {
    let wrong = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Изменилось размещение, код или состав рынка LunarBase",
        )
    };
    if ctx.chain_id() != Some(8453)
        || ctx.dex_addr("dex.pmm_pool", id)? != POOL
        || ctx.dex_addr("dex.pool_implementation", id)? != IMPL
    {
        return Err(wrong());
    }
    let word: B256 = rpc::call(
        ctx,
        "eth_getStorageAt",
        json!([POOL, super::abi_woo::implementation_slot(), tag]),
    )
    .await?;
    if Address::from_word(word) != IMPL {
        return Err(wrong());
    }
    for (addr, expected) in [
        (
            POOL,
            b256!("0281e85a65fd03a885a66dadaf6d9dbeacad82e3e3f98a8106a678d5d4299936"),
        ),
        (
            IMPL,
            b256!("3fecc332729c2ab166dd413d97200e2ab11953132c7fc8962aef3bc837c3ef6c"),
        ),
    ] {
        let bytes: Bytes = rpc::call(ctx, "eth_getCode", json!([addr, tag])).await?;
        if keccak256(bytes) != expected {
            return Err(wrong());
        }
    }
    let (x, y) = tokio::try_join!(
        view(ctx, tag, Address::ZERO, POOL, ILunar::XCall {}),
        view(ctx, tag, Address::ZERO, POOL, ILunar::YCall {})
    )?;
    if x != Address::ZERO || y != USDC {
        return Err(wrong());
    }
    Ok(())
}

pub async fn verify(ctx: &Ctx, c: &Candidate, address: Address) -> GatewayResult<Found> {
    if address != POOL || !pair(c.coin, c.quote) {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Для этого рынка LunarBase нет подтверждения native ETH/USDC",
        ));
    }
    let tag = super::elfomo::block(ctx, "latest").await?;
    verify_at(ctx, c.family.id, &tag).await?;
    Ok(verify_woo::found(c, PoolRef::Contract(address)))
}

pub fn topics() -> Vec<B256> {
    use alloy::sol_types::SolEvent;
    vec![
        ILunar::SwapExecuted::SIGNATURE_HASH,
        ILunar::Upgraded::SIGNATURE_HASH,
        ILunar::StateUpdated::SIGNATURE_HASH,
        ILunar::Sync::SIGNATURE_HASH,
        ILunar::Paused::SIGNATURE_HASH,
        ILunar::Unpaused::SIGNATURE_HASH,
        ILunar::WhitelistSet::SIGNATURE_HASH,
        ILunar::PartnerFeeSet::SIGNATURE_HASH,
        ILunar::BlacklistFeeMultiplierSet::SIGNATURE_HASH,
        ILunar::BlockDelaySet::SIGNATURE_HASH,
        ILunar::MaxPunishmentX24Set::SIGNATURE_HASH,
    ]
}
