//! Код и фактические роли публичного роутера или отдельного проверенного модуля на одном блоке.
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        exec::flux_eth::Market,
        node::{call, rpc},
        pools::{exact_wallet_tokens, flux_native, registry::Entry},
    },
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, keccak256};
use serde_json::{Value, json};
alloy::sol! {interface IRouter {
    function clPoolManager() external view returns(address);
    function binPoolManager() external view returns(address);
    function vault() external view returns(address);
    function paused() external view returns(bool);
}}
pub(super) fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Omni Flux: нативное поручение, код или разрешение не подтверждены",
    )
}
pub(super) async fn view<C: SolCall>(
    ctx: &Ctx,
    at: &Value,
    target: Address,
    c: C,
) -> GatewayResult<C::Return> {
    let bytes: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":target,"data":Bytes::from(c.abi_encode())},at]),
    )
    .await?;
    call::decode::<C>(&bytes)
}
async fn code(ctx: &Ctx, id: &str, target: Address, key: &str, at: &Value) -> GatewayResult<()> {
    let hash: B256 = ctx.dex_text(key, id)?.parse().map_err(|_| bad(ctx))?;
    let raw: Bytes = rpc::call(ctx, "eth_getCode", json!([target, at])).await?;
    if raw.is_empty() || keccak256(raw) != hash {
        return Err(bad(ctx));
    }
    Ok(())
}
pub(in crate::venues::bsc_dex) async fn checked(
    ctx: &Ctx,
    e: &Entry,
    at: &Value,
    module: bool,
) -> GatewayResult<Market> {
    let generation = ctx.settings.generation();
    let m = Market::read(ctx, e, module)?;
    if module {
        tokio::try_join!(
            flux_native::module(ctx, e, at),
            code(
                ctx,
                e.family.id,
                m.router,
                "dex.infinity_native_module_hash",
                at
            )
        )?;
        if ctx.settings.generation() != generation {
            return Err(bad(ctx));
        }
        return Ok(m);
    }
    let f = e.family;
    let check_manager = async {
        if m.bin {
            view(ctx, at, m.router, IRouter::binPoolManagerCall {}).await
        } else {
            view(ctx, at, m.router, IRouter::clPoolManagerCall {}).await
        }
    };
    let (found, (), (), (), manager, vault, paused) = tokio::try_join!(
        flux_native::market(ctx, f, m.manager, m.id, (m.wrapped, m.token), at),
        exact_wallet_tokens::check_input(ctx, f.id, m.token, at),
        code(ctx, f.id, m.router, "dex.router_runtime_hash", at),
        code(ctx, f.id, m.permit, "dex.permit2_runtime_hash", at),
        check_manager,
        view(ctx, at, m.router, IRouter::vaultCall {}),
        view(ctx, at, m.router, IRouter::pausedCall {})
    )?;
    if found.key != e.key
        || manager != m.manager
        || vault != ctx.dex_addr("dex.vault", f.id)?
        || paused
        || ctx.settings.generation() != generation
    {
        return Err(bad(ctx));
    }
    Ok(m)
}
