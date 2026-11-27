//! Загружает лестницы обеих сторон и доступный многошаговый модуль на каноническом блоке.
use super::{
    deployment_at_head::Cache,
    families::Family,
    fluid_dex_lite::{bad, bind, resolve},
    fluid_lite_quotes::levels,
    ladder_counter,
    registry::Registry,
};
use crate::{
    core::types::{PoolRef, PoolState, fluid_dex_lite::State},
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::call, quote::white_state::Head, tokens::Tokens},
};
use alloy_primitives::{Address, B256, U256};
use std::sync::LazyLock;

#[allow(clippy::too_many_arguments)]
pub(super) async fn load(
    c: &Ctx,
    t: &Tokens,
    reg: &Registry,
    f: &Family,
    reference: &PoolRef,
    pair: (Address, Address),
    counter: Address,
    h: Head,
) -> GatewayResult<PoolState> {
    let PoolRef::Singleton { manager, id } = reference else {
        return Err(bad(c));
    };
    let at = h.selector();
    let r = bind(c, f, &at).await?;
    if *manager != r.core {
        return Err(bad(c));
    }
    let market = resolve(c, r.core, *id, &at).await?;
    let wrapped = c.addr("dex.native_wrapped")?;
    let coins = market.tokens(wrapped).ok_or_else(|| bad(c))?;
    if pair.0 == pair.1 || !coins.contains(&pair.0) || !coins.contains(&pair.1) {
        return Err(bad(c));
    }
    let idx = coins
        .iter()
        .position(|v| *v == counter)
        .ok_or_else(|| bad(c))?;
    let (mut a, mut b, di, dout) = tokio::try_join!(
        ladder_counter::amounts(c, t, reg, f.id, counter),
        ladder_counter::amounts(c, t, reg, f.id, coins[1 - idx]),
        t.decimals(c, counter),
        t.decimals(c, coins[1 - idx])
    )?;
    // The original math rejects fixed amounts under 10_000 at its nine-decimal precision.
    // Sample that boundary explicitly; a shallow genuine pool may reject every configured level.
    let minimum = |dec: u8| {
        U256::from(10)
            .checked_pow(U256::from(dec.saturating_sub(5)))
            .ok_or_else(|| bad(c))
    };
    a.push(minimum(di)?);
    a.sort_unstable();
    a.dedup();
    b.push(minimum(dout)?);
    let buys = levels(c, r, market, idx == 0, &a, &at).await?;
    b.extend(buys.iter().map(|p| p.amount_out));
    b.sort();
    b.dedup();
    let sells = levels(c, r, market, idx != 0, &b, &at).await?;
    let module = c.dex_addr("dex.router", f.id).ok().filter(|v| !v.is_zero());
    let hop_module = if let Some(target) = module
        && (!buys.is_empty() || !sells.is_empty())
    {
        type VersionKey = (B256, Address, Address, Address, B256, Vec<B256>, u64);
        static VERSION: LazyLock<Cache<bool, VersionKey>> = LazyLock::new(Default::default);
        let pins = super::direct_module::configured(c, f.id).unwrap_or_default();
        let cell = VERSION.at((
            h.hash,
            target,
            wrapped,
            r.core,
            r.hash,
            pins,
            c.settings.generation(),
        ));
        let valid=cell.get_or_init(||async{
            use crate::venues::bsc_dex::exec::fluid_lite_wallet::abi::IWallet;
            matches!(tokio::try_join!(super::direct_module::verify_code(c,f.id,target,&at),call::canonical(c,&at,target,IWallet::hopVersionCall{}),call::canonical(c,&at,target,IWallet::dexCall{}),call::canonical(c,&at,target,IWallet::wrappedCall{}),call::canonical(c,&at,target,IWallet::venueHashCall{})),Ok(((),v,d,w,ch)) if v==U256::ONE && (d,w,ch)==(r.core,wrapped,r.hash))
        }).await;
        valid.then_some(target)
    } else {
        None
    };
    Ok(PoolState::FluidDexLite(Box::new(State {
        stamp: reg.ambient_stamp(h.number.to(), h.hash),
        dex: r.core,
        resolver: r.resolver,
        market,
        wrapped,
        module,
        hop_module,
        levels: if idx == 0 {
            [buys, sells]
        } else {
            [sells, buys]
        },
    })))
}
