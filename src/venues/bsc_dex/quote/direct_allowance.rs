//! Получатель ERC20-разрешения для прямого обмена. Денежный план и котировка используют один диспетчер рынка.
use super::super::{
    ctx::Ctx,
    exec::market_approval,
    pools::{deltaswap, family_variant::Variant, registry::Entry},
};
use crate::{core::types::RouterFeeMode, gateway::errors::GatewayResult};
use alloy_primitives::Address;

pub(super) fn spender(
    ctx: &Ctx,
    e: &Entry,
    mode: Option<RouterFeeMode>,
    native: bool,
) -> GatewayResult<Option<Address>> {
    let configured = |key| ctx.dex_addr(key, e.family.id).map(Some);
    if let Some(spender) = market_approval::spender(ctx, e, mode) {
        return spender.map(Some);
    }
    let oasis = super::oasis_pricing::is_oasis(e);
    let frax = super::frax_pricing::is_frax(e);
    let ekubo = super::ekubo_v2_pricing::enabled(e);
    let vault = super::super::exec::vault_direct::enabled(e);
    if !(super::vista_pricing::native_router(e)
        || oasis
        || frax
        || super::white_pricing::is_white(e)
        || super::router_pricing::is_native(e)
        || ekubo
        || vault
        || super::super::pools::sectorone::enabled(e)
        || super::delta_pricing::is_delta(e))
    {
        return Ok(None);
    }
    if e.family.variant == Variant::Surge {
        return market_approval::signed(ctx, e, mode).map(Some);
    }
    if ekubo {
        return configured("dex.quoter");
    }
    if vault {
        return configured("dex.vault");
    }
    let gross = matches!(
        mode,
        Some(
            RouterFeeMode::NativeGrossMinimum
                | RouterFeeMode::NativeSettlement {
                    gross_minimum: true
                }
        )
    );
    let bunny_native = e.family.variant == Variant::BunnySwap
        && (native || matches!(mode, Some(RouterFeeMode::NativeSettlement { .. })));
    if gross || bunny_native || oasis || frax {
        return configured("dex.quoter");
    }
    deltaswap::router(ctx, e).map(Some)
}

pub(super) fn permit_router(
    ctx: &Ctx,
    e: &Entry,
    mode: crate::core::types::RouteMode,
    fees: Option<RouterFeeMode>,
) -> GatewayResult<Option<(Address, Address)>> {
    if mode == crate::core::types::RouteMode::Direct && e.family.variant == Variant::SlipstreamUniversal {
        let router=ctx.dex_addr("dex.universal_router",e.family.id)?;
        if !matches!(fees,Some(RouterFeeMode::ProtectedWallet{target}) if target==router) {
            return Err(ctx.err(crate::gateway::errors::ErrorCode::NoRoute,"Slipstream: выбранный UniversalRouter изменился"));
        }
        return Ok(Some((router,ctx.dex_addr("dex.permit2",e.family.id)?)));
    }
    if mode != crate::core::types::RouteMode::Direct || e.family.variant != Variant::PuzzleRanges {
        return Ok(None);
    }
    let (market, router) = super::super::exec::ranges_wallet::Market::read(ctx, e)?;
    if !matches!(fees,Some(RouterFeeMode::ProtectedWallet{target}) if target==router) {
        return Err(ctx.err(
            crate::gateway::errors::ErrorCode::BadResponse,
            "Ranges: для Permit2 не подтверждён выбранный роутер",
        ));
    }
    Ok(Some((router, market.permit)))
}
