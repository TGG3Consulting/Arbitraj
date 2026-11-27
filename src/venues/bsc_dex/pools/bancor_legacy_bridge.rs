//! Составной Network-path связывает два конкретных оригинала одной физической валютой.
use super::{
    bancor_legacy::{self, ILegacy, Roles},
    families::Family,
    nami_batch as batch,
};
use crate::{
    core::types::{
        PoolRef,
        bancor_legacy::{BancorLegacyBridge, BancorLegacyState},
    },
    gateway::errors::GatewayResult,
    venues::bsc_dex::{ctx::Ctx, node::rpc},
};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub(super) fn parse(s: &str) -> Option<PoolRef> {
    match s.split_once('/') {
        Some((a, b)) => {
            let a = a.parse::<Address>().ok()?;
            let b = b.parse::<Address>().ok()?;
            (a != b && !a.is_zero() && !b.is_zero()).then_some(PoolRef::Singleton {
                manager: a,
                id: b.into_word(),
            })
        }
        None => Some(PoolRef::Contract(s.parse().ok()?)),
    }
}
pub(crate) async fn bound(
    ctx: &Ctx,
    f: &Family,
    reference: &PoolRef,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<BancorLegacyState> {
    if let PoolRef::Contract(c) = reference {
        return bancor_legacy::market(ctx, f, *c, pair, at).await;
    }
    let PoolRef::Singleton { manager, id } = reference else {
        return Err(bancor_legacy::bad(ctx));
    };
    if id[..12] != [0u8; 12] {
        return Err(bancor_legacy::bad(ctx));
    }
    let second = Address::from_slice(&id[12..]);
    if second == *manager {
        return Err(bancor_legacy::bad(ctx));
    }
    let r = Roles::read(ctx, f, at).await?;
    let (mut a, b) = tokio::try_join!(
        bancor_legacy::single(ctx, &r, *manager, at),
        bancor_legacy::single(ctx, &r, second, at)
    )?;
    let (Some(i), Some(j)) = (
        a.tokens.iter().position(|t| *t == pair.0),
        b.tokens.iter().position(|t| *t == pair.1),
    ) else {
        return Err(bancor_legacy::bad(ctx));
    };
    let middle = a.reserves[1 - i];
    if a.anchor == b.anchor
        || middle == r.native
        || middle != b.reserves[1 - j]
        || a.tokens[1 - i] == pair.1
        || b.tokens[1 - j] == pair.0
        || pair.0 >= pair.1
    {
        return Err(bancor_legacy::bad(ctx));
    }
    a.second = Some(BancorLegacyBridge {
        middle,
        anchor: b.anchor,
        converter: b.converter,
        fee_ppm: b.fee_ppm,
    });
    a.tokens = [pair.0, pair.1];
    a.reserves = [a.reserves[i], b.reserves[j]];
    a.balances = [a.balances[i], b.balances[j]];
    a.wallet_balances = [a.wallet_balances[i], b.wallet_balances[j]];
    Ok(a)
}
// На каждую наблюдаемую валюту сохраняются только её фактические смежные конвертеры.
// Пересечение индексов по промежуточному ERC20 не создаёт декартово произведение всех реестров.
pub(super) type Neighbours = HashMap<Address, HashMap<Address, HashSet<Address>>>;
pub(super) async fn edges(
    ctx: &Ctx,
    r: &Roles,
    converters: &[Address],
    at: &Value,
) -> GatewayResult<Neighbours> {
    let mut map = Neighbours::new();
    let limit = ctx.int("dex.verify_batch")?.max(1);
    for group in converters.chunks(usize::try_from(limit).unwrap_or(usize::MAX)) {
        let mut calls = Vec::with_capacity(group.len() * 3);
        for &c in group {
            calls.push(batch::view(at, c, ILegacy::connectorTokenCountCall {}));
            for i in [U256::ZERO, U256::ONE] {
                calls.push(batch::view(at, c, ILegacy::connectorTokensCall { i }));
            }
        }
        let rows = rpc::batch(ctx, &calls).await?;
        if rows.len() != calls.len() {
            return Err(bancor_legacy::bad(ctx));
        }
        for (&c, parts) in group.iter().zip(rows.chunks_exact(3)) {
            let bytes = |i: usize| -> Option<Bytes> {
                serde_json::from_str(parts[i].as_ref().ok()?.get()).ok()
            };
            let Some(n) = bytes(0) else { continue };
            if batch::decode(&n, ILegacy::connectorTokenCountCall {}).ok() != Some(2) {
                continue;
            }
            let (Some(a), Some(b)) = (bytes(1), bytes(2)) else {
                continue;
            };
            let decode = |x| batch::decode(x, ILegacy::connectorTokensCall { i: U256::ZERO }).ok();
            let (Some(a), Some(b)) = (decode(&a), decode(&b)) else {
                continue;
            };
            if a.is_zero() || b.is_zero() || r.normalize(a) == r.normalize(b) {
                continue;
            }
            for (src, dst) in [(a, b), (b, a)] {
                map.entry(r.normalize(src))
                    .or_default()
                    .entry(dst)
                    .or_default()
                    .insert(c);
            }
        }
    }
    Ok(map)
}
pub(super) fn paths(map: &Neighbours, a: Address, b: Address, r: &Roles) -> Vec<String> {
    let (a, b) = super::discover::sorted(a, b);
    let (Some(left), Some(right)) = (map.get(&a), map.get(&b)) else {
        return vec![];
    };
    let mut routes = HashSet::new();
    for (middle, pools) in left {
        if r.normalize(*middle) == b {
            routes.extend(pools.iter().map(|p| format!("{p:#x}")));
            continue;
        }
        if *middle == r.native || r.normalize(*middle) == a {
            continue;
        }
        if let Some(other) = right.get(middle) {
            for p in pools {
                for q in other {
                    if p != q {
                        routes.insert(format!("{p:#x}/{q:#x}"));
                    }
                }
            }
        }
    }
    let mut routes: Vec<_> = routes.into_iter().collect();
    routes.sort_unstable();
    routes
}
