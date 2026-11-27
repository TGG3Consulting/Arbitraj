//! Нативный Omni Flux с расчётом WETH. Реестр и модуль подтверждают один физический ключ
//! и одну пару активов на общем каноническом блоке. token0 наблюдаемого рынка остаётся ETH в PoolKey.
use super::{
    abi_check::IInfinityKeys,
    direct_module,
    discover::{Found, sorted},
    discover_v4,
    families::{Family, Proto},
    key::{
        InfinityPoolKeyData, PoolKey, infinity_bin_step, infinity_hook_bits, infinity_tick_spacing,
    },
    nami_batch as batch,
    verify::{Candidate, Verdict},
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256, keccak256};
use serde_json::{Value, json};
alloy::sol! { interface IFlux {
    function fluxCl() external view returns(address);
    function fluxBin() external view returns(address);
    function fluxVault() external view returns(address);
    function wrapped() external view returns(address);
    function vault() external view returns(address);
    function fluxMarket(bytes32 id,address token) external view returns(address currency0,address currency1,address hooks,address manager,uint24 fee,bytes32 parameters);
}}
fn invalid(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::QuoteMismatch,
        "Omni Flux: код, роли, физический PoolId или актив расчёта расходятся с настройками",
    )
}
pub(super) fn economic(ctx: &Ctx, mut f: Found) -> GatewayResult<Option<Found>> {
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let Some(PoolKey::Infinity(k)) = &f.key else {
        return Ok(None);
    };
    if k.manager != ctx.dex_addr("dex.pool_manager", f.family.id)? || !accepts(k, wrapped) {
        return Ok(None);
    }
    f.token0 = wrapped;
    f.coin = wrapped;
    Ok(Some(f))
}
fn accepts(k: &InfinityPoolKeyData, wrapped: Address) -> bool {
    !wrapped.is_zero()
        && k.currency0.is_zero()
        && !k.currency1.is_zero()
        && k.currency1 != wrapped
        && k.hooks.is_zero()
        && infinity_hook_bits(k.parameters) == 0
}
pub(in crate::venues::bsc_dex) async fn market(
    ctx: &Ctx,
    f: &'static Family,
    manager: Address,
    id: B256,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<Found> {
    let generation = ctx.settings.generation();
    let wrapped = ctx.addr("dex.native_wrapped")?;
    let module = ctx.dex_addr("dex.router", f.id)?;
    let vault = ctx.dex_addr("dex.vault", f.id)?;
    let quoter = ctx.dex_addr("dex.quoter", f.id)?;
    if manager != ctx.dex_addr("dex.pool_manager", f.id)?
        || !f.variant.flux_native()
        || pair.0 == pair.1
    {
        return Err(invalid(ctx));
    }
    let token = if pair.0 == wrapped {
        pair.1
    } else if pair.1 == wrapped {
        pair.0
    } else {
        return Err(invalid(ctx));
    };
    let cl = f.proto == Proto::InfinityCl;
    let role = if cl {
        batch::view(at, module, IFlux::fluxClCall {})
    } else {
        batch::view(at, module, IFlux::fluxBinCall {})
    };
    let get = IInfinityKeys::poolIdToPoolKeyCall { id };
    let mk = IFlux::fluxMarketCall { id, token };
    let views = [
        role,
        batch::view(at, module, IFlux::fluxVaultCall {}),
        batch::view(at, module, IFlux::wrappedCall {}),
        batch::view(at, manager, IFlux::vaultCall {}),
        batch::view(at, manager, get.clone()),
        batch::view(at, module, mk),
    ];
    let roles = [manager, vault, quoter];
    let hashes = ctx.dex_list("dex.infinity_runtime_hashes", f.id)?;
    let requests = roles.map(|a| ("eth_getCode", json!([a, at])));
    let ((), [m, v, w, actual_vault, raw, reply], codes) = tokio::try_join!(
        direct_module::verify_code(ctx, f.id, module, at),
        batch::read(ctx, views),
        batch::read(ctx, requests)
    )?;
    if hashes.len() != roles.len()
        || !hashes
            .iter()
            .zip(codes)
            .all(|(h, code)| !code.is_empty() && h.parse::<B256>().ok() == Some(keccak256(code)))
    {
        return Err(invalid(ctx));
    }
    let role = if cl {
        batch::decode(&m, IFlux::fluxClCall {})?
    } else {
        batch::decode(&m, IFlux::fluxBinCall {})?
    };
    if role != manager
        || batch::decode(&v, IFlux::fluxVaultCall {})? != vault
        || batch::decode(&w, IFlux::wrappedCall {})? != wrapped
        || batch::decode(&actual_vault, IFlux::vaultCall {})? != vault
        || reply != raw
        || raw.len() != 192
        || keccak256(&raw) != id
    {
        return Err(invalid(ctx));
    }
    let k = batch::decode(&raw, get)?;
    let key = InfinityPoolKeyData {
        currency0: k.currency0,
        currency1: k.currency1,
        hooks: k.hooks,
        manager: k.poolManager,
        fee: k.fee.to(),
        parameters: k.parameters,
    };
    if key.manager != manager
        || token != key.currency1
        || !accepts(&key, wrapped)
        || ctx.settings.generation() != generation
    {
        return Err(invalid(ctx));
    }
    let spacing = cl.then(|| infinity_tick_spacing(key.parameters));
    let step = (!cl).then(|| infinity_bin_step(key.parameters));
    let found = discover_v4::from_key(f, manager, id, PoolKey::Infinity(key), spacing, step);
    economic(ctx, found)?.ok_or_else(|| invalid(ctx))
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let fetch = async {
        let manager = ctx.dex_addr("dex.pool_manager", c.family.id)?;
        let id = c.pool.parse().map_err(|_| invalid(ctx))?;
        let at = super::nami::block(ctx, "latest").await?;
        let found = market(ctx, c.family, manager, id, sorted(c.coin, c.quote), &at).await?;
        Ok::<_, crate::gateway::errors::GatewayError>(Found {
            coin: c.coin,
            ..found
        })
    };
    match fetch.await {
        Ok(f) => Verdict::Taken(Box::new(f)),
        Err(e) if e.code == ErrorCode::QuoteMismatch => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
