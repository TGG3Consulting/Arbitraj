//! Файл: venues/bsc_dex/pools/lunar.rs.
//! Что делает: сверяет активный proxy LunarBase, реализацию, порядок native ETH/USDC и код общего модуля.
//! Кто вызывает: поиск, загрузка, квотер, подготовка сделки. От чего зависит: ABI и архив lunarbase.json.
//! Настройки: dex.kind, dex.pmm_pool, dex.pool_implementation, dex.router и dex.pmm_ladder_usd.
use super::super::ctx::Ctx;
use super::{
    discover::Found, families::Proto, pmm_profile::Profile, verify::Candidate, verify_woo,
};
use crate::core::types::PoolRef;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy_primitives::{Address, B256};
use serde_json::Value;

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
pub(super) async fn open(
    ctx: &Ctx,
    tag: &Value,
    owner: Address,
    pool: Address,
) -> GatewayResult<bool> {
    let (paused, fresh) = tokio::try_join!(
        view(ctx, tag, owner, pool, ILunar::pausedCall {}),
        view(ctx, tag, owner, pool, ILunar::isFreshCall {})
    )?;
    Ok(!paused && fresh)
}

pub(super) async fn verify_at(ctx: &Ctx, profile: &Profile, tag: &Value) -> GatewayResult<()> {
    let wrong = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Изменилось размещение, код или состав рынка LunarBase",
        )
    };
    profile.verify_code(ctx, tag).await?;
    let (x, y) = tokio::try_join!(
        view(ctx, tag, Address::ZERO, profile.pool, ILunar::XCall {}),
        view(ctx, tag, Address::ZERO, profile.pool, ILunar::YCall {})
    )?;
    if x != Address::ZERO || y != profile.counter {
        return Err(wrong());
    }
    Ok(())
}

pub async fn verify(ctx: &Ctx, c: &Candidate, address: Address) -> GatewayResult<Found> {
    let profile = Profile::read(ctx, c.family.id, Proto::Lunar)?;
    if address != profile.pool || !profile.pair(c.coin, c.quote) {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Для этого рынка LunarBase нет подтверждения native ETH/USDC",
        ));
    }
    let tag = super::elfomo::block(ctx, "latest").await?;
    verify_at(ctx, &profile, &tag).await?;
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
