//! Quote and run each leg on the actual book and probe wallet. The second leg sees the first receipt's order balances.
use super::{
    super::{
        ctx::Ctx,
        exec::{carbon_direct, plan::Order},
        math::carbon_match,
        node::{abi::IERC20, call, rpc},
        pools::{
            carbon::{self, ICarbon},
            carbon_quote,
            registry::Entry,
        },
        quote::white_state::Head,
        wallet::approve::{self, Approval},
    },
    simulate::SimLog,
};
use crate::{
    core::types::{CarbonOrder, CarbonState, PoolRef, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::{Address, Bytes, LogData, U256};
use serde_json::json;

fn fail(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Carbon: фактические изменения заявок или доступное финансирование пробы не подтверждены",
    )
}
pub(super) async fn funded(
    ctx: &Ctx,
    s: &CarbonState,
    input: Address,
    amount: U256,
    payer: Address,
    h: Head,
) -> GatewayResult<()> {
    let (physical, _) = carbon_quote::contracts(s, input).ok_or_else(|| fail(ctx))?;
    let available = if physical == carbon::NATIVE {
        rpc::call(ctx, "eth_getBalance", json!([payer, h.selector()])).await?
    } else {
        let q = IERC20::balanceOfCall { owner: payer };
        let raw: Bytes = rpc::call(
            ctx,
            "eth_call",
            json!([{"to":physical,"data":Bytes::from(q.abi_encode())},h.selector()]),
        )
        .await?;
        call::decode::<IERC20::balanceOfCall>(&raw)?
    };
    if available < amount {
        return Err(ctx.err(
            ErrorCode::InsufficientBalance,
            "Carbon: на кошельке пробы недостаточно исходной валюты в выбранном блоке",
        ));
    }
    Ok(())
}
pub(super) async fn leg(
    ctx: &Ctx,
    e: &Entry,
    o: &Order,
    h: Head,
    now: u64,
) -> GatewayResult<(Vec<Approval>, Address, Bytes)> {
    let PoolState::Carbon(s) = &e.pool.state else {
        return Err(fail(ctx));
    };
    let (mut c, _) = carbon_direct::call(
        ctx,
        s,
        o.token_in,
        false,
        o.amount_in,
        o.amount_in,
        o.min_out,
        now,
    )?;
    if c.output != o.token_out {
        return Err(fail(ctx));
    }
    let protect = c.target != carbon::NATIVE;
    carbon_direct::bound(ctx, e, &mut c, h, protect).await?;
    let target = c.recipient();
    let approvals = if c.source == carbon::NATIVE {
        vec![]
    } else {
        approve::fork_all(ctx, c.input, target, c.cap())?
    };
    Ok((approvals, target, c.data()))
}

fn update(log: &SimLog) -> Option<ICarbon::StrategyUpdated> {
    if log.topics.first() != Some(&ICarbon::StrategyUpdated::SIGNATURE_HASH) {
        return None;
    }
    ICarbon::StrategyUpdated::decode_log_data(&LogData::new(log.topics.clone(), log.data.clone())?)
        .ok()
}
pub(super) fn emitted(s: &CarbonState, r: &PoolRef, log: &SimLog) -> bool {
    let PoolRef::Singleton { manager, id } = r else {
        return false;
    };
    if *manager != s.controller || log.address != *manager {
        return false;
    }
    let Some(u) = update(log) else {
        return false;
    };
    let t = carbon::raw(s);
    u.reason == 1
        && (u.id >> 128usize) == U256::from_be_bytes(id.0)
        && ([u.token0, u.token1] == t || [u.token1, u.token0] == t)
}

pub(super) fn updated(
    s: &CarbonState,
    r: &PoolRef,
    input: Address,
    amount: U256,
    logs: &[SimLog],
) -> Option<CarbonState> {
    let p = carbon_match::plan(s, input, false, u128::try_from(amount).ok()?)?;
    let raw = carbon::raw(s);
    let mut next = s.clone();
    let mut actions = p.actions.iter();
    for log in logs.iter().filter(|l| {
        l.address == s.controller
            && l.topics.first() == Some(&ICarbon::StrategyUpdated::SIGNATURE_HASH)
    }) {
        if !emitted(s, r, log) {
            return None;
        }
        let u = update(log)?;
        let order = actions.next()?;
        if order.id != u.id {
            return None;
        }
        let mut orders = [u.order0, u.order1].map(|o| CarbonOrder {
            y: o.y,
            z: o.z,
            a: o.a,
            b: o.b,
        });
        if orders.iter().any(|o| o.y > o.z) {
            return None;
        }
        if u.token0 != raw[0] {
            orders.swap(0, 1);
        }
        next.strategies.iter_mut().find(|s| s.id == u.id)?.orders = orders;
    }
    (actions.len() == 0).then_some(next)
}
pub(super) fn after(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
    logs: &[SimLog],
) -> GatewayResult<Option<Box<Entry>>> {
    let PoolState::Carbon(s) = &e.pool.state else {
        return Ok(None);
    };
    let state = updated(s, &e.pool.reference, input, amount, logs).ok_or_else(|| fail(ctx))?;
    let mut next = Box::new(e.clone());
    next.pool.state = PoolState::Carbon(Box::new(state));
    Ok(Some(next))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::CarbonStrategy;
    use flate2::read::GzDecoder;
    use serde_json::Value;
    fn read<T: std::str::FromStr>(v: &Value) -> T
    where
        T::Err: std::fmt::Debug,
    {
        v.as_str().unwrap().parse().unwrap()
    }
    #[test]
    fn archived_buy_updates_the_same_strategy_used_for_the_following_sell() {
        let bytes = include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/carbon_ethereum.json.gz"
        );
        let raw: Value = serde_json::from_reader(GzDecoder::new(bytes.as_slice())).unwrap();
        let address = read::<Address>;
        let weth = alloy_primitives::address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
        let normalized = |a| if a == carbon::NATIVE { weth } else { a };
        let mut checked = 0;
        for case in raw["controls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["by_target"] == false)
        {
            let id = read::<U256>(&case["id"]);
            let pair = u128::try_from(id >> 128usize).unwrap();
            let market = &raw["directory"][pair as usize - 1];
            let t = &market["tokens"];
            let (tokens, native_side) =
                carbon::normalized([address(&t[0]), address(&t[1])], Some(weth)).unwrap();
            let row = market["orders"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r[0] == case["id"])
                .unwrap();
            let order = |i: usize| {
                let o = &row[3][i];
                CarbonOrder {
                    y: read(&o[0]),
                    z: read(&o[1]),
                    a: read(&o[2]),
                    b: read(&o[3]),
                }
            };
            let mut orders = [order(0), order(1)];
            if normalized(address(&row[2][0])) != tokens[0] {
                orders.swap(0, 1);
            }
            let s = CarbonState {
                controller: address(&raw["roles"][0]),
                tokens,
                native_side,
                fee_ppm: market["fee_ppm"].as_u64().unwrap().try_into().unwrap(),
                strategies: vec![CarbonStrategy { id, orders }],
            };
            let reference = PoolRef::Singleton {
                manager: s.controller,
                id: carbon::pair_id(pair),
            };
            let logs: Vec<SimLog> = serde_json::from_value(case["logs"].clone()).unwrap();
            let input = normalized(address(&case["source"]));
            let amount = read::<U256>(&case["input"]);
            let p = carbon_match::plan(&s, input, false, u128::try_from(amount).unwrap()).unwrap();
            let expected = carbon_match::after(&s, input, &p).unwrap();
            let observed = updated(&s, &reference, input, amount, &logs).unwrap();
            for i in 0..2 {
                let a = &observed.strategies[0].orders[i];
                let b = &expected.strategies[0].orders[i];
                assert_eq!((a.y, a.z, a.a, a.b), (b.y, b.z, b.a, b.b));
            }
            assert_eq!(p.received, read::<u128>(&case["output"]));
            let other = PoolRef::Singleton {
                manager: s.controller,
                id: carbon::pair_id(pair + 1),
            };
            assert!(updated(&s, &other, input, amount, &logs).is_none());
            assert!(updated(&s, &reference, input, amount, &[]).is_none());
            checked += 1;
        }
        assert_eq!(checked, 6);
    }
}
