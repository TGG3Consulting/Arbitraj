//! Ключи рынков launchpad из Initialize и настроек. Полное совпадение пяти ABI-полей
//! определяет PoolId. Разрешённый хук и его текущие зависимости сверяет launch_market.
use super::{
    abi_check::IPositionManagerKeys,
    families::Family,
    family_variant::Variant,
    key::{PoolKey, UniswapPoolKeyData},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, keccak256};

pub(super) fn invalid(ctx: &Ctx) -> GatewayError {
    ctx.err(
        ErrorCode::QuoteMismatch,
        "Launchpad: ключ, актив, контракт или роль не подтверждены на блоке",
    )
}

#[derive(Clone)]
pub(super) struct Role {
    pub kind: String,
    pub address: Address,
    pub hash: B256,
}

pub(super) fn roles(ctx: &Ctx, f: &Family) -> GatewayResult<Vec<Role>> {
    let mut out = Vec::new();
    for text in ctx.dex_list("dex.launch_roles", f.id)? {
        let mut parts = text.split('@');
        let kind = parts.next().ok_or_else(|| invalid(ctx))?;
        let addr = parts
            .next()
            .and_then(|s| s.parse::<Address>().ok())
            .ok_or_else(|| invalid(ctx))?;
        let hash = parts
            .next()
            .and_then(|s| s.parse::<B256>().ok())
            .ok_or_else(|| invalid(ctx))?;
        if !matches!(
            kind,
            "hook" | "after_hook" | "factory" | "escrow" | "registry" | "airlock"
        ) || parts.next().is_some()
            || addr.is_zero()
            || hash.is_zero()
            || hash == keccak256([])
            || out.iter().any(|r: &Role| r.address == addr)
        {
            return Err(invalid(ctx));
        }
        out.push(Role {
            kind: kind.to_owned(),
            address: addr,
            hash,
        });
    }
    Ok(out)
}

fn shape(f: &Family, key: &PoolKey) -> bool {
    let PoolKey::Uniswap(k) = key else {
        return false;
    };
    k.currency0 < k.currency1
        && !k.hooks.is_zero()
        && match f.variant {
            Variant::LaunchDoppler => k.fee == 0x800000 && k.tick_spacing.as_i32() == 200,
            Variant::LaunchIndex => k.fee == 0 && k.tick_spacing.as_i32() == 1,
            Variant::LaunchO1 => k.fee == 0 && k.tick_spacing.as_i32() == 200,
            _ => false,
        }
}

pub(super) fn configured(ctx: &Ctx, f: &Family, id: B256) -> GatewayResult<PoolKey> {
    let mut selected = None;
    for s in ctx.dex_list("dex.launch_pool_keys", f.id)? {
        let raw: Bytes = s.parse().map_err(|_| invalid(ctx))?;
        if keccak256(&raw) != id {
            continue;
        }
        if selected.is_some() || raw.len() != 160 {
            return Err(invalid(ctx));
        }
        let k = IPositionManagerKeys::poolKeysCall::abi_decode_returns_validate(&raw)
            .map_err(|_| invalid(ctx))?;
        selected = Some(PoolKey::Uniswap(UniswapPoolKeyData {
            currency0: k.currency0,
            currency1: k.currency1,
            fee: k.fee.to(),
            tick_spacing: k.tickSpacing,
            hooks: k.hooks,
        }));
    }
    selected
        .or_else(|| super::deli_keys::preimage(id))
        .filter(|k| shape(f, k))
        .ok_or_else(|| invalid(ctx))
}

pub(super) fn observed(
    ctx: &Ctx,
    f: &Family,
    manager: Address,
    k: &PoolKey,
) -> GatewayResult<bool> {
    if manager != ctx.dex_addr("dex.pool_manager", f.id)?
        || !shape(f, k)
        || !roles(ctx, f)?
            .iter()
            .any(|r| r.kind == "hook" && r.address == k.hooks())
    {
        return Ok(false);
    }
    super::deli_keys::remember(ctx, k.clone())?;
    Ok(true)
}
