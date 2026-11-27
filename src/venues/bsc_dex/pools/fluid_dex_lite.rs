//! Instadapp/fluid-contracts-public@404d73f91e6806f881f8dabeb30494f93f9e99b7, deployments/mainnet/{FluidDexLite,FluidDexLiteResolver}.json.
use super::{
    balancer_v1::{data, individual, views},
    deployment_at_head::{Cache, Key},
    families::Family,
    ladder_counter,
    registry::{Entry, Registry},
};
use crate::{
    core::types::{
        LadderPoint, PoolRef, PoolState,
        fluid_dex_lite::{Market, State},
    },
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{call, rpc},
        quote::white_state::Head,
        tokens::Tokens,
    },
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, I256, U256, keccak256};
use serde_json::{Value, json};
use std::sync::LazyLock;
sol! {
    struct DexKey {address token0;address token1;bytes32 salt;}
    interface IFluidLite{
        function readFromStorage(bytes32 slot) external view returns(uint256);
        function estimateSwapHop(address[] path,DexKey[] keys,int256 specified) external returns(uint256);
        function estimateSwapSingle(DexKey pool,bool forward,int256 specified) external returns(uint256);
    }
}
pub(crate) fn bad(c: &Ctx) -> GatewayError {
    c.err(
        ErrorCode::QuoteMismatch,
        "Fluid DEX Lite: отсутствует подтверждение исходного рынка, кода или суммы",
    )
}
#[derive(Clone, Copy)]
pub(crate) struct Roles {
    pub core: Address,
    pub resolver: Address,
    pub hash: B256,
}
static ROLES: LazyLock<Cache> = LazyLock::new(Default::default);
pub(crate) async fn bind(c: &Ctx, f: &Family, at: &Value) -> GatewayResult<Roles> {
    let dex = c.dex_addr("dex.factory", f.id)?;
    let resolver = c.dex_addr("dex.quoter", f.id)?;
    let configured = |name| -> GatewayResult<B256> {
        c.dex_text(name, f.id)?
            .trim()
            .parse()
            .ok()
            .filter(|h: &B256| !h.is_zero() && *h != keccak256([]))
            .ok_or_else(|| bad(c))
    };
    let hash = configured("dex.factory_runtime_hash")?;
    let lens = configured("dex.quoter_runtime_hash")?;
    let block = at["blockHash"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .filter(|_| at["requireCanonical"] == true)
        .ok_or_else(|| bad(c))?;
    let cell = ROLES.at(Key {
        block,
        contracts: [(dex, hash), (resolver, lens)],
    });
    cell.get_or_try_init(|| async {
        let (_, code) = tokio::try_join!(
            super::solidly_classic_factory::plain(c, dex, hash, at),
            rpc::call::<Bytes>(c, "eth_getCode", json!([resolver, at]))
        )?;
        // Official resolver: the first DEX_LITE immutable is PUSH32 at PC 1016 (hardhat runtime template vs on-chain instantiation).
        if keccak256(&code) != lens
            || code.get(1016) != Some(&0x7f)
            || code.get(1017..1049) != Some(dex.into_word().as_slice())
        {
            return Err(bad(c));
        }
        Ok(())
    })
    .await?;
    Ok(Roles {
        core: dex,
        resolver,
        hash,
    })
}
fn slot(dex: Address, index: U256) -> (Address, Bytes) {
    data(dex, IFluidLite::readFromStorageCall { slot: index.into() })
}
pub(super) async fn count(c: &Ctx, dex: Address, at: &Value) -> GatewayResult<U256> {
    call::canonical(
        c,
        at,
        dex,
        IFluidLite::readFromStorageCall {
            slot: U256::ONE.into(),
        },
    )
    .await
}
static START: LazyLock<U256> =
    LazyLock::new(|| U256::from_be_bytes(keccak256(U256::ONE.to_be_bytes::<32>()).0));
pub(super) async fn keys(
    c: &Ctx,
    dex: Address,
    idx: &[U256],
    at: &Value,
) -> GatewayResult<Vec<Market>> {
    let mut requests = Vec::new();
    for &n in idx {
        let pos = n
            .checked_mul(U256::from(3))
            .and_then(|v| v.checked_add(*START))
            .ok_or_else(|| bad(c))?;
        for j in 0..3u8 {
            requests.push(slot(
                dex,
                pos.checked_add(U256::from(j)).ok_or_else(|| bad(c))?,
            ))
        }
    }
    let raw = views(c, &requests, at).await?;
    if raw.len() != requests.len() {
        return Err(bad(c));
    }
    let addr = |v: &Bytes| -> Option<Address> {
        (v.len() == 32 && v[..12].iter().all(|x| *x == 0)).then(|| Address::from_slice(&v[12..]))
    };
    raw.chunks_exact(3)
        .zip(idx)
        .map(|(a, &index)| {
            Ok(Market {
                token0: addr(&a[0]).ok_or_else(|| bad(c))?,
                token1: addr(&a[1]).ok_or_else(|| bad(c))?,
                salt: B256::try_from(a[2].as_ref()).map_err(|_| bad(c))?,
                index,
            })
        })
        .collect()
}
pub(crate) async fn resolve(c: &Ctx, dex: Address, id: B256, at: &Value) -> GatewayResult<Market> {
    let idx = U256::from_be_slice(&id[8..]);
    let (n, rows) = tokio::try_join!(
        count(c, dex, at),
        keys(c, dex, std::slice::from_ref(&idx), at)
    )?;
    rows.first()
        .copied()
        .filter(|v| idx < n && v.id() == Some(id))
        .ok_or_else(|| bad(c))
}
pub(crate) fn shape(e: &Entry) -> Option<&State> {
    let PoolState::FluidDexLite(s) = &e.pool.state else {
        return None;
    };
    let PoolRef::Singleton { manager, id } = e.pool.reference else {
        return None;
    };
    let pair = s.market.tokens(s.wrapped)?;
    (e.family.variant == super::family_variant::Variant::FluidDexLite
        && e.pool.kind == crate::core::types::PoolKind::V2
        && manager == s.dex
        && !s.dex.is_zero()
        && !s.resolver.is_zero()
        && s.market.id() == Some(id)
        && pair.contains(&e.pool.token0)
        && pair.contains(&e.pool.token1)
        && e.pool.token0 != e.pool.token1)
        .then_some(s)
}
pub(crate) async fn check(c: &Ctx, e: &Entry, at: &Value) -> GatewayResult<Roles> {
    let s = shape(e).ok_or_else(|| bad(c))?;
    let r = bind(c, e.family, at).await?;
    if s.wrapped != c.addr("dex.native_wrapped")?
        || s.dex != r.core
        || s.resolver != r.resolver
        || resolve(c, r.core, s.market.id().ok_or_else(|| bad(c))?, at).await? != s.market
    {
        return Err(bad(c));
    }
    Ok(r)
}
fn source(
    resolver: Address,
    m: Market,
    forward: bool,
    qty: U256,
    exact_in: bool,
) -> Option<(Address, Bytes)> {
    if qty.is_zero() {
        return None;
    }
    let raw = I256::try_from(qty).ok()?;
    let specified = if exact_in { raw } else { raw.checked_neg()? };
    Some(data(
        resolver,
        IFluidLite::estimateSwapSingleCall {
            pool: DexKey {
                token0: m.token0,
                token1: m.token1,
                salt: m.salt,
            },
            forward,
            specified,
        },
    ))
}
fn point(bytes: &Bytes, qty: U256, exact_in: bool) -> Option<LadderPoint> {
    let other = IFluidLite::estimateSwapSingleCall::abi_decode_returns_validate(bytes).ok()?;
    if other.is_zero() {
        return None;
    }
    let (amount_in, amount_out) = if exact_in { (qty, other) } else { (other, qty) };
    Some(LadderPoint {
        amount_in,
        amount_out,
    })
}
pub(crate) async fn official(
    c: &Ctx,
    e: &Entry,
    input: Address,
    qty: U256,
    exact_in: bool,
    at: &Value,
) -> GatewayResult<LadderPoint> {
    let s = shape(e).ok_or_else(|| bad(c))?;
    let r = check(c, e, at).await?;
    let forward = s.side(input).ok_or_else(|| bad(c))? == 0;
    let call = source(r.resolver, s.market, forward, qty, exact_in).ok_or_else(|| bad(c))?;
    let raw = views(c, &[call], at).await?;
    raw.first()
        .and_then(|b| point(b, qty, exact_in))
        .ok_or_else(|| bad(c))
}
async fn levels(
    c: &Ctx,
    r: Roles,
    m: Market,
    forward: bool,
    inputs: &[U256],
    at: &Value,
) -> GatewayResult<Vec<LadderPoint>> {
    let qs: Vec<_> = inputs
        .iter()
        .filter_map(|&n| Some((n, source(r.resolver, m, forward, n, true)?)))
        .collect();
    let calls: Vec<_> = qs.iter().map(|(_, q)| q.clone()).collect();
    let rows = individual(c, &calls, at).await?;
    if rows.len() != qs.len() {
        return Err(bad(c));
    }
    let mut points: Vec<_> = qs
        .into_iter()
        .zip(rows)
        .filter_map(|((n, _), raw)| point(&raw.ok()?, n, true))
        .collect();
    points.sort_by_key(|p| p.amount_in);
    points.dedup_by_key(|p| p.amount_in);
    Ok(points)
}
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

pub(crate) fn request(
    c: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let s = shape(e).ok_or_else(|| bad(c))?;
    source(
        s.resolver,
        s.market,
        s.side(input).ok_or_else(|| bad(c))? == 0,
        amount,
        true,
    )
    .ok_or_else(|| bad(c))
}
pub(crate) fn payout(e: &Entry, input: Address, amount: U256, raw: &[u8]) -> Option<U256> {
    let s = shape(e)?;
    s.side(input)?;
    if amount.is_zero() {
        return None;
    }
    IFluidLite::estimateSwapSingleCall::abi_decode_returns_validate(raw)
        .ok()
        .filter(|v| !v.is_zero())
}

pub(crate) async fn hop_quote(
    ctx: &Ctx,
    resolver: Address,
    path: Vec<Address>,
    markets: &[Market],
    amount: U256,
    exact_in: bool,
    at: &Value,
) -> GatewayResult<U256> {
    let keys = markets
        .iter()
        .map(|m| DexKey {
            token0: m.token0,
            token1: m.token1,
            salt: m.salt,
        })
        .collect();
    let qty = I256::try_from(amount).map_err(|_| bad(ctx))?;
    let specified = if exact_in {
        qty
    } else {
        qty.checked_neg().ok_or_else(|| bad(ctx))?
    };
    call::canonical(
        ctx,
        at,
        resolver,
        IFluidLite::estimateSwapHopCall {
            path,
            keys,
            specified,
        },
    )
    .await
}

impl Registry {
    pub(in crate::venues::bsc_dex) fn multi_market_edges(&self) -> Vec<std::sync::Arc<Entry>> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let mut edges: Vec<_> = inner
            .path_refs
            .iter()
            .filter_map(|r| inner.get(r))
            .filter(|e| {
                e.valid
                    && !e.asleep
                    && (matches!(&e.pool.state,crate::core::types::PoolState::Motoswap(s) if (s.path_module.is_some() || s.fee_router_frozen) && crate::venues::bsc_dex::exec::motoswap_plan::shape(e).is_some()) || shape(e).is_some_and(|s| {
                        s.stamp.current()
                            && s.hop_module.is_some()
                            && s.levels.iter().any(|l| !l.is_empty())
                    }))
            })
            .cloned()
            .collect();
        edges.sort_by_key(|e| (e.pool.reference.address(),shape(e).and_then(|s|s.market.id())));
        edges
    }
}
