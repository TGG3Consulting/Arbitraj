//! Адрес прямого роутера и проверка исполнителя, привязанного к фабрике Umbrae.
use super::{
    families::Family, family_variant::Variant, nami_batch as batch, umbrae_abi::IMarket,
    virtuals_roles::Role,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::Address;
use serde_json::Value;
pub(crate) fn native(ctx: &Ctx, f: &Family) -> Option<Address> {
    let rows = ctx.dex_list("dex.umbrae_roles", f.id).ok()?;
    let [_, router, _] = rows.as_slice() else {
        return None;
    };
    Some(Role::parse(router)?.address)
}

pub(crate) async fn module(ctx: &Ctx, f: &Family, addr: Address, at: &Value) -> GatewayResult<()> {
    let gen_before = ctx.settings.generation();
    super::direct_module::verify_code(ctx, f.id, addr, at).await?;
    let which = if f.variant == Variant::UmbraeDlmm {
        batch::view(at, addr, IMarket::dlmmCall {})
    } else {
        batch::view(at, addr, IMarket::dammCall {})
    };
    let [raw] = batch::read(ctx, [which]).await?;
    if batch::decode(&raw, IMarket::dlmmCall {})? != ctx.dex_addr("dex.factory", f.id)?
        || gen_before != ctx.settings.generation()
    {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "Модуль Umbrae связан с другой фабрикой либо настройки изменились",
        ));
    }
    Ok(())
}
