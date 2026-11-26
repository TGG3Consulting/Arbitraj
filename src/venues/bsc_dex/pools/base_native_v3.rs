//! Файл: venues/bsc_dex/pools/base_native_v3.rs.
//! Что делает: сверяет настройку квотера и его ABI с архивной приёмкой для выбранной фабрики.
//! Кто вызывает: допуск семейства, котировка и сборка обмена через прямой модуль.
//! Настройки: dex.factory, dex.quoter, dex.quoter_virtual, dex.v3_quoter_method.
use super::{super::ctx::Ctx, deployment_profile, families::Family};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy_primitives::Address;

pub fn mixed_quoter(ctx: &Ctx, family: &Family) -> GatewayResult<(Address, bool)> {
    let wrong = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Фабрика или ABI V3-квотера не соответствуют приёмке для сети",
        )
    };
    let factory = ctx.dex_addr("dex.factory", family.id)?;
    let row = ctx
        .chain_id()
        .and_then(|id| deployment_profile::for_factory(id, factory, "base_native_v3"))
        .ok_or_else(wrong)?;
    let expected = &row["settings"];
    let address = ctx.dex_addr("dex.quoter", family.id)?;
    let method = ctx.dex_text("dex.v3_quoter_method", family.id)?;
    if expected["dex.quoter"]
        .as_str()
        .and_then(|s| s.parse::<Address>().ok())
        != Some(address)
        || expected["dex.v3_quoter_method"].as_str() != Some(method.as_str())
    {
        return Err(wrong());
    }
    match method.as_str() {
        "quoteExactInputSingle" => Ok((address, false)),
        "quoteExactInputSingleV3" => Ok((address, true)),
        _ => Err(wrong()),
    }
}

pub fn valid(ctx: &Ctx, family: &Family) -> bool {
    !ctx.dex_flag("dex.quoter_virtual", family.id)
        .unwrap_or(false)
        && mixed_quoter(ctx, family).is_ok()
}
