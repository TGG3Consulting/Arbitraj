//! Carbon v1 controller version 6. Pair IDs are distinct from strategy IDs and from both token addresses.
use super::super::ctx::Ctx;
use super::{families::Family, nami_batch as batch};
use crate::{
    core::types::{CarbonOrder, CarbonState, CarbonStrategy},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::sol;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};
use std::collections::HashSet;

sol! {
    struct BookOrder { uint128 y;uint128 z;uint64 a;uint64 b; }
    struct BookStrategy {uint256 id;address owner;address[2] tokens;BookOrder[2] orders;}
    struct Pair {uint128 id;address[2] tokens;}
    interface ICarbon {
        function version() external view returns(uint16);
        function pairs() external view returns(address[2][] memory);
        function pair(address token0,address token1) external view returns(Pair memory);
        function pairTradingFeePPM(address token0,address token1) external view returns(uint32);
        function strategiesByPairCount(address token0,address token1) external view returns(uint256);
        function strategiesByPair(address token0,address token1,uint256 startIndex,uint256 endIndex) external view returns(BookStrategy[] memory);
        event PairCreated(uint128 indexed pairId,address indexed token0,address indexed token1);
        event StrategyCreated(uint256 id,address indexed owner,address indexed token0,address indexed token1,BookOrder order0,BookOrder order1);
        event StrategyDeleted(uint256 id,address indexed owner,address indexed token0,address indexed token1,BookOrder order0,BookOrder order1);
        event StrategyUpdated(uint256 indexed id,address indexed token0,address indexed token1,BookOrder order0,BookOrder order1,uint8 reason);
        event TokensTraded(address indexed trader,address indexed sourceToken,address indexed targetToken,uint256 sourceAmount,uint256 targetAmount,uint128 tradingFeeAmount,bool byTargetAmount);
        event TradingFeePPMUpdated(uint32 prevFeePPM,uint32 newFeePPM);
        event PairTradingFeePPMUpdated(address indexed token0,address indexed token1,uint32 prevFeePPM,uint32 newFeePPM);
        event Upgraded(address indexed implementation);
    }
}
pub const NATIVE: Address = Address::repeat_byte(0xee);
pub fn pair_id(id: u128) -> B256 {
    B256::from(U256::from(id).to_be_bytes::<32>())
}
/// Pairs._pairsStorage comes from verified Controller v6 storageLayout (slot 53); pair() independently checks its result.
pub fn pair_slots(id: B256) -> [B256; 2] {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(id.as_slice());
    input[63] = 53;
    let first = keccak256(input);
    [
        first,
        B256::from(
            U256::from_be_bytes(first.0)
                .wrapping_add(U256::ONE)
                .to_be_bytes::<32>(),
        ),
    ]
}
pub fn normalized(
    raw: [Address; 2],
    wrapped: Option<Address>,
) -> Option<([Address; 2], Option<usize>)> {
    if raw[0].is_zero() || raw[0] >= raw[1] {
        return None;
    }
    let mut pair = raw;
    let native = raw.contains(&NATIVE);
    if native {
        let w = wrapped.filter(|a| !a.is_zero() && *a != NATIVE)?;
        for a in &mut pair {
            if *a == NATIVE {
                *a = w;
            }
        }
    }
    if pair[0] == pair[1] {
        return None;
    }
    pair.sort_unstable();
    let native_side = if native {
        Some(pair.iter().position(|x| Some(*x) == wrapped)?)
    } else {
        None
    };
    Some((pair, native_side))
}
pub(crate) fn raw(s: &CarbonState) -> [Address; 2] {
    let mut pair = s.tokens;
    if let Some(i) = s.native_side
        && let Some(slot) = pair.get_mut(i)
    {
        *slot = NATIVE;
    }
    pair
}
pub(super) async fn controller(ctx: &Ctx, f: &Family, at: &Value) -> GatewayResult<Address> {
    if ctx.dex_list("dex.proxy_implementations", f.id)?.len() != 1 {
        return Err(ctx.err(
            ErrorCode::NotConfigured,
            "Carbon: требуется подтверждённая реализация контроллера",
        ));
    }
    super::solidly_classic_factory::at(ctx, f, at).await
}
fn order(o: BookOrder) -> CarbonOrder {
    CarbonOrder {
        y: o.y,
        z: o.z,
        a: o.a,
        b: o.b,
    }
}
pub(super) fn orient(r: BookStrategy, pair: u128, tokens: [Address; 2]) -> Option<CarbonStrategy> {
    if r.owner.is_zero() || r.id >> 128 != U256::from(pair) || r.orders.iter().any(|o| o.y > o.z) {
        return None;
    }
    let mut orders = r.orders.map(order);
    if r.tokens == [tokens[1], tokens[0]] {
        orders.swap(0, 1)
    } else if r.tokens != tokens {
        return None;
    }
    Some(CarbonStrategy { id: r.id, orders })
}
pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    controller_address: Address,
    id: B256,
    expected: (Address, Address),
    at: &Value,
) -> GatewayResult<CarbonState> {
    let generation = ctx.settings.generation();
    let bad = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Carbon: идентификатор пары, контракт или заявки отличаются от канонического реестра",
        )
    };
    if id.is_zero()
        || id[..16].iter().any(|v| *v != 0)
        || ctx.dex_addr("dex.factory", f.id)? != controller_address
    {
        return Err(bad());
    }
    let read = |slot: B256| ("eth_getStorageAt", json!([controller_address, slot, at]));
    let [lo, hi] = pair_slots(id);
    let (_, data) = tokio::try_join!(
        controller(ctx, f, at),
        batch::read(
            ctx,
            [
                read(lo),
                read(hi),
                batch::view(at, controller_address, ICarbon::versionCall {})
            ]
        )
    )?;
    let [lr, hr, ver] = data;
    let addr = |b: Bytes| {
        if b.len() == 32 && b[..12].iter().all(|v| *v == 0) {
            Some(Address::from_slice(&b[12..]))
        } else {
            None
        }
    };
    let a = addr(lr).ok_or_else(bad)?;
    let b = addr(hr).ok_or_else(bad)?;
    let raw_pair = [a, b];
    let (tokens, native_side) =
        normalized(raw_pair, ctx.addr("dex.native_wrapped").ok()).ok_or_else(bad)?;
    if super::discover::sorted(expected.0, expected.1) != (tokens[0], tokens[1])
        || batch::decode(&ver, ICarbon::versionCall {})? != 6
    {
        return Err(bad());
    }
    let lookup = ICarbon::pairCall {
        token0: a,
        token1: b,
    };
    let fee = ICarbon::pairTradingFeePPMCall {
        token0: a,
        token1: b,
    };
    let count = ICarbon::strategiesByPairCountCall {
        token0: a,
        token1: b,
    };
    let [p, g, n] = batch::read(
        ctx,
        [
            batch::view(at, controller_address, lookup.clone()),
            batch::view(at, controller_address, fee.clone()),
            batch::view(at, controller_address, count.clone()),
        ],
    )
    .await?;
    let found = batch::decode(&p, lookup)?;
    let fee_ppm = batch::decode(&g, fee)?;
    let count = usize::try_from(batch::decode(&n, count)?).map_err(|_| bad())?;
    if pair_id(found.id) != id || found.tokens != raw_pair || fee_ppm >= 1_000_000 {
        return Err(bad());
    }
    let mut state = CarbonState {
        controller: controller_address,
        tokens,
        native_side,
        fee_ppm,
        strategies: Vec::new(),
    };
    let oriented = raw(&state);
    let mut unique = HashSet::new();
    // Each ABI page is bounded to 32 strategies. Independent pages share one RPC round trip.
    for base in (0..count).step_by(256) {
        let pages:Vec<_>=(base..base.saturating_add(256).min(count)).step_by(32).map(|start|{
            let end=start.saturating_add(32).min(count);
            (end-start,ICarbon::strategiesByPairCall{token0:a,token1:b,startIndex:U256::from(start),endIndex:U256::from(end)})
        }).collect();
        let requests:Vec<_>=pages.iter().map(|(_,q)|batch::view(at,controller_address,q.clone())).collect();
        let replies=super::super::node::rpc::batch(ctx,&requests).await?;
        if replies.len()!=pages.len(){return Err(bad());}
        for ((length,call),reply) in pages.into_iter().zip(replies){
            let part:Bytes=serde_json::from_str(reply?.get()).map_err(|_|bad())?;
            let rows=batch::decode(&part,call)?;
            if rows.len()!=length{return Err(bad());}
            for r in rows{if !unique.insert(r.id){return Err(bad());}state.strategies.push(orient(r,found.id,oriented).ok_or_else(bad)?);}
        }
    }
    if ctx.settings.generation() != generation {
        return Err(bad());
    }
    Ok(state)
}
