//! Привязка рынка SectorOne к реестру и исполняемому клону на одном блоке.
use super::{
    abi_lb::{ILBFactory, ILBPair},
    abi_lb20::ILBPairV20,
    abi_lb22::ILBPair22,
    families::Proto,
    family_variant::Variant,
    nami_batch as batch,
    registry::Entry,
    verify_crypto_v1, verify_lb, verify_lb22,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, quote::router_pricing},
};
use alloy_primitives::{Address, U256, keccak256};
use serde_json::{Value, json};

pub(in crate::venues::bsc_dex) fn enabled(e: &Entry) -> bool {
    e.family.variant == Variant::SectorLb
}

pub(in crate::venues::bsc_dex) fn bin(e: &Entry) -> Option<(bool, u32)> {
    let step = e
        .pool
        .bin_step
        .filter(|&n| n > 0 && n <= u32::from(u16::MAX))?;
    Some((e.family.proto == Proto::LbV20, step))
}

pub(in crate::venues::bsc_dex) async fn check(
    ctx: &Ctx,
    e: &Entry,
    at: &Value,
    native: bool,
) -> GatewayResult<()> {
    let invalid = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "SectorOne: не совпали клон, токены, фабрика, хук либо роутер",
        )
    };
    let generation = ctx.settings.generation();
    let f = e.family;
    let (legacy, step) = bin(e).ok_or_else(invalid)?;
    let factory = ctx.dex_addr("dex.factory", f.id)?;
    let pool = e.pool.reference.address();
    let registry = ILBFactory::getLBPairInformationCall {
        tokenA: e.pool.token0,
        tokenB: e.pool.token1,
        binStep: U256::from(step),
    };
    let owner = if legacy {
        batch::view(at, pool, ILBPairV20::factoryCall {})
    } else {
        batch::view(at, pool, ILBPair::getFactoryCall {})
    };
    let [code, registered, owner] = batch::read(
        ctx,
        [
            ("eth_getCode", json!([pool, at])),
            batch::view(at, factory, registry.clone()),
            owner,
        ],
    )
    .await?;
    let implementation = if legacy {
        verify_crypto_v1::proxy_target(&code)
    } else {
        verify_lb::clone_implementation(&code)
    }
    .ok_or_else(invalid)?;
    let accepted =
        verify_lb22::settings(ctx, f).map_err(|e| ctx.err(ErrorCode::NotConfigured, e))?;
    if !accepted.contains(&implementation)
        || implementation.is_zero()
        || batch::decode(&registered, registry)?.LBPair != pool
        || batch::decode(&owner, ILBPair::getFactoryCall {})? != factory
    {
        return Err(invalid());
    }
    if !legacy {
        let c = ILBPair22::getLBHooksParametersCall {};
        let [raw] = batch::read(ctx, [batch::view(at, pool, c.clone())]).await?;
        let flags = U256::from_be_bytes(batch::decode(&raw, c)?.0);
        if flags.bit(160) || flags.bit(161) {
            return Err(invalid());
        }
    }
    if native || legacy {
        let (router, hash) = router_pricing::profile(ctx, f.id)?;
        if legacy && ctx.dex_addr("dex.quoter", f.id)? != router {
            return Err(invalid());
        }
        let target = if legacy {
            batch::view(at, router, ILBPairV20::factoryCall {})
        } else {
            batch::view(at, router, ILBPair::getFactoryCall {})
        };
        let [code, owner] =
            batch::read(ctx, [("eth_getCode", json!([router, at])), target]).await?;
        if code.is_empty()
            || keccak256(code) != hash
            || batch::decode(&owner, ILBPair::getFactoryCall {})? != factory
        {
            return Err(invalid());
        }
    }
    if generation != ctx.settings.generation() {
        return Err(invalid());
    }
    Ok(())
}
