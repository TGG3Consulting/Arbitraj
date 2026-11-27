//! Для Deli CL PositionManager хранит ключ реальной позиции. Комиссия в wBLT читается
//! из slot0 каждого блока; нулевой LP fee ядра задаёт сам хук в beforeSwap.
use super::{
    abi_check::IPositionManagerKeys,
    deli::{checked_roles, invalid},
    deli_keys,
    families::Family,
    key::{PoolKey, UniswapPoolKeyData},
    load::Loaded,
    load_cl, nami_batch as batch,
};
use crate::gateway::errors::GatewayResult;
use crate::venues::bsc_dex::{ctx::Ctx, quote::white_state};
use alloy_primitives::{Address, B256, FixedBytes};
use serde_json::Value;
alloy::sol! { interface IDeliCurrency { function WBLT() external view returns(address); } }

pub(in crate::venues::bsc_dex) async fn check(
    ctx: &Ctx,
    f: &Family,
    manager: Address,
    id: B256,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<i32> {
    let generation = ctx.settings.generation();
    let hook = deli_keys::bindings(ctx, f)?[0].0;
    let pos = ctx.dex_addr("dex.position_manager", f.id)?;
    let asset = ctx.dex_addr("dex.deli_fee_currency", f.id)?;
    if pair.0 != asset && pair.1 != asset {
        return Err(invalid(ctx));
    }
    let call = IPositionManagerKeys::poolKeysCall {
        poolId: FixedBytes::<25>::from_slice(&id[..25]),
    };
    let mgr = IPositionManagerKeys::poolManagerCall {};
    let wblt = IDeliCurrency::WBLTCall {};
    let reads = [
        batch::view(at, pos, call.clone()),
        batch::view(at, pos, mgr.clone()),
        batch::view(at, hook, wblt.clone()),
    ];
    let ((), [k, m, w]) = tokio::try_join!(
        checked_roles(ctx, f, manager, hook, id, at),
        batch::read(ctx, reads)
    )?;
    let k = batch::decode(&k, call)?;
    let position_key = PoolKey::Uniswap(UniswapPoolKeyData {
        currency0: k.currency0,
        currency1: k.currency1,
        fee: k.fee.to(),
        tick_spacing: k.tickSpacing,
        hooks: k.hooks,
    });
    // У официального PositionManager нет записи, пока через него не создана позиция.
    // Прообраз из Initialize принимаем только при полностью нулевом ключе; live slot0 и роли проверены выше.
    let key = if k.currency0.is_zero()
        && k.currency1.is_zero()
        && k.fee.is_zero()
        && k.tickSpacing.is_zero()
        && k.hooks.is_zero()
    {
        deli_keys::preimage(id).ok_or_else(|| invalid(ctx))?
    } else {
        position_key
    };
    let PoolKey::Uniswap(k) = &key else {
        return Err(invalid(ctx));
    };
    if generation != ctx.settings.generation()
        || batch::decode(&m, mgr)? != manager
        || batch::decode(&w, wblt)? != asset
        || k.hooks != hook
        || (k.currency0, k.currency1) != pair
        || key.id() != id
        || key.fee() != 0x800000
        || k.tick_spacing.as_i32() <= 0
    {
        return Err(invalid(ctx));
    }
    Ok(k.tick_spacing.as_i32())
}

pub(super) async fn load(
    ctx: &Ctx,
    f: &Family,
    manager: Address,
    id: B256,
    pair: (Address, Address),
    tag: &str,
) -> GatewayResult<Loaded> {
    let generation = ctx.settings.generation();
    let h = white_state::head(ctx, tag).await?;
    let fixed = format!("{:#x}", h.number);
    let spacing = check(ctx, f, manager, id, pair, &h.selector()).await?;
    let (state, ticks) = load_cl::load(ctx, f, manager, id, spacing, &fixed).await?;
    if ctx.settings.generation() != generation
        || white_state::head(ctx, &fixed).await?.hash != h.hash
    {
        return Err(invalid(ctx));
    }
    Ok(Loaded {
        state,
        ticks: Some(ticks),
        bins: None,
        fee_pips: None,
    })
}
