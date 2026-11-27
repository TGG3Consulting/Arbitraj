//! Angstrom L1. Сверяет физический ключ Uniswap V4 с действующим ConfigStore на одном блоке.
use super::{
    families::Family,
    key::PoolKey,
    launch_abi::{ILaunch, bound},
    launch_keys::{Role, invalid},
    nami_batch as batch,
};
use crate::{gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, FixedBytes, keccak256};
use serde_json::{Value, json};

alloy::sol! {
    interface IInspector {
        function angstrom() external view returns (address);
        function uniswapPoolManager() external view returns (address);
        function configStore() external view returns (address);
        function unlockedFee(bytes27 key) external view returns (uint24 lp, uint24 protocol);
    }
}

pub(in crate::venues::bsc_dex) fn identity(
    ctx: &Ctx,
    f: &Family,
    id: alloy_primitives::B256,
) -> GatewayResult<(PoolKey, Address)> {
    let key = super::launch_keys::configured(ctx, f, id)?;
    let adapter = unique(ctx, &super::launch_keys::roles(ctx, f)?, "adapter")?;
    Ok((key, adapter))
}

pub(in crate::venues::bsc_dex) async fn market(
    ctx: &Ctx,
    f: &Family,
    manager: Address,
    id: alloy_primitives::B256,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<(PoolKey, Address)> {
    let g = ctx.settings.generation();
    let market = super::launch_market::market(ctx, f, manager, id, pair, at).await?;
    let role = unique(ctx, &super::launch_keys::roles(ctx, f)?, "adapter")?;
    if g != ctx.settings.generation() {
        return Err(invalid(ctx));
    }
    Ok((market.key, role))
}

fn unique(ctx: &Ctx, roles: &[Role], kind: &str) -> GatewayResult<Address> {
    let mut selected = roles.iter().filter(|r| r.kind == kind);
    let address = selected.next().ok_or_else(|| invalid(ctx))?.address;
    if selected.next().is_some() {
        return Err(invalid(ctx));
    }
    Ok(address)
}

fn registered(code: &[u8], key: FixedBytes<27>, spacing: i32) -> bool {
    let Some((&0, rows)) = code.split_first() else {
        return false;
    };
    let entries = rows.chunks_exact(32);
    if !entries.remainder().is_empty() {
        return false;
    }
    let mut found = entries.filter(|row| row[..27] == key[..]);
    let Some(row) = found.next() else {
        return false;
    };
    i32::from(u16::from_be_bytes([row[27], row[28]])) == spacing && found.next().is_none()
}

pub(super) async fn terms(
    ctx: &Ctx,
    f: &Family,
    key: &PoolKey,
    roles: &[Role],
    at: &Value,
) -> GatewayResult<(Address, bool, bool)> {
    let PoolKey::Uniswap(k) = key else {
        return Err(invalid(ctx));
    };
    let inspector = unique(ctx, roles, "inspector")?;
    let adapter = unique(ctx, roles, "adapter")?;
    let manager = ctx.dex_addr("dex.pool_manager", f.id)?;
    let hash = keccak256((k.currency0, k.currency1).abi_encode_params());
    let store_key = FixedBytes::<27>::from_slice(&hash[5..]);
    let hook = IInspector::angstromCall {};
    let pool_manager = IInspector::uniswapPoolManagerCall {};
    let adapter_manager = ILaunch::poolManagerCall {};
    let config = IInspector::configStoreCall {};
    let fees = IInspector::unlockedFeeCall { key: store_key };
    let calls = [
        batch::view(at, inspector, hook.clone()),
        batch::view(at, inspector, pool_manager.clone()),
        batch::view(at, adapter, adapter_manager.clone()),
        batch::view(at, inspector, config.clone()),
        batch::view(at, inspector, fees.clone()),
    ];
    let ((), (), [h, m, a, s, u]) = tokio::try_join!(
        bound(ctx, roles, "inspector", inspector, at),
        bound(ctx, roles, "adapter", adapter, at),
        batch::read(ctx, calls)
    )?;
    let store = batch::decode(&s, config)?;
    let fee = batch::decode(&u, fees)?;
    if batch::decode(&h, hook)? != k.hooks
        || batch::decode(&m, pool_manager)? != manager
        || batch::decode(&a, adapter_manager)? != manager
        || store.is_zero()
        || fee.lp.to::<u32>() > 1_000_000
        || fee.protocol.to::<u32>() >= 1_000_000
    {
        return Err(invalid(ctx));
    }
    let [program] = batch::read(ctx, [("eth_getCode", json!([store, at]))]).await?;
    if !registered(&program, store_key, k.tick_spacing.as_i32()) {
        return Err(invalid(ctx));
    }
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let asset = if k.currency0 == wrapped {
        k.currency0
    } else {
        k.currency1
    };
    Ok((asset, true, true))
}
