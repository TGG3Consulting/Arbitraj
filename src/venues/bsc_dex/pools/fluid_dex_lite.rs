//! Читает канонический каталог и сверяет ключ, runtime кассы и resolver Fluid DEX Lite.
//! Источник: Instadapp/fluid-contracts-public@404d73f91e6806f881f8dabeb30494f93f9e99b7, deployments/mainnet/FluidDexLite.json.
pub(super) use super::fluid_lite_levels::load;
pub(crate) use super::fluid_lite_quotes::{hop_quote, official, payout, request};
use super::{
    balancer_v1::{data, views},
    deployment_at_head::{Cache, Key},
    families::Family,
    registry::Entry,
};
use crate::{
    core::types::{
        PoolRef, PoolState,
        fluid_dex_lite::{Market, State},
    },
    gateway::errors::{ErrorCode, GatewayError, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{call, rpc},
    },
};
use alloy::sol;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
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
    (e.family.variant.fluid_lite()
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
