//! Official Carbon v6 getters settle the same ordered (strategyId,amount) vector selected by the local quote.
use super::super::{
    ctx::Ctx,
    math::carbon_match::{self, Plan},
    node::rpc,
};
use super::{
    carbon::{self, ICarbon, TradeAction},
    registry::Entry,
};
use crate::{
    core::types::{CarbonState, PoolState},
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256};
use serde_json::{Value, json};

pub(crate) fn contracts(s: &CarbonState, input: Address) -> Option<(Address, Address)> {
    if s.native_side.is_some_and(|i| i >= s.tokens.len()) {
        return None;
    }
    let raw = carbon::raw(s);
    if input == s.tokens[0] {
        Some((raw[0], raw[1]))
    } else if input == s.tokens[1] {
        Some((raw[1], raw[0]))
    } else {
        None
    }
}
fn payload(s: &CarbonState, input: Address, p: &Plan) -> Option<Bytes> {
    let (source, target) = contracts(s, input)?;
    let actions = p
        .actions
        .iter()
        .map(|a| TradeAction {
            strategyId: a.id,
            amount: a.amount,
        })
        .collect();
    Some(if p.by_target {
        ICarbon::calculateTradeSourceAmountCall {
            sourceToken: source,
            targetToken: target,
            actions,
        }
        .abi_encode()
        .into()
    } else {
        ICarbon::calculateTradeTargetAmountCall {
            sourceToken: source,
            targetToken: target,
            actions,
        }
        .abi_encode()
        .into()
    })
}
fn decoded(p: &Plan, data: &[u8]) -> Option<u128> {
    let (actual, expected) = if p.by_target {
        (
            ICarbon::calculateTradeSourceAmountCall::abi_decode_returns_validate(data).ok()?,
            p.spent,
        )
    } else {
        (
            ICarbon::calculateTradeTargetAmountCall::abi_decode_returns_validate(data).ok()?,
            p.received,
        )
    };
    (actual == expected && actual > 0).then_some(actual)
}
pub(crate) async fn official(
    ctx: &Ctx,
    s: &CarbonState,
    input: Address,
    p: &Plan,
    at: &Value,
) -> GatewayResult<u128> {
    let bad = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "Carbon: официальный расчёт распределения по стратегиям расходится с локальным",
        )
    };
    if at["requireCanonical"] != true
        || at["blockHash"]
            .as_str()
            .and_then(|v| v.parse::<B256>().ok())
            .is_none()
    {
        return Err(bad());
    }
    let data = payload(s, input, p).ok_or_else(bad)?;
    let raw: Bytes =
        rpc::call(ctx, "eth_call", json!([{"to":s.controller,"data":data},at])).await?;
    decoded(p, &raw).ok_or_else(bad)
}
fn quote(e: &Entry, input: Address, amount: U256) -> Option<(&CarbonState, Plan)> {
    let PoolState::Carbon(s) = &e.pool.state else {
        return None;
    };
    let amount = u128::try_from(amount).ok()?;
    let p = carbon_match::plan(s, input, false, amount)?;
    (p.spent == amount).then_some((s, p))
}
pub(crate) fn request(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Carbon: недостаточно исполнимых заявок на полную сумму",
        )
    };
    let (s, p) = quote(e, input, amount).ok_or_else(bad)?;
    let data = payload(s, input, &p).ok_or_else(bad)?;
    Ok((s.controller, data))
}
pub(crate) fn payout(e: &Entry, input: Address, amount: U256, raw: &[u8]) -> Option<U256> {
    let (_, p) = quote(e, input, amount)?;
    decoded(&p, raw).map(U256::from)
}

#[cfg(test)]
mod tests {
    use super::super::super::math::carbon_match::Action;
    use super::*;
    #[test]
    fn two_strategy_official_getters_use_the_exact_allocation_and_aggregate_fee() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/carbon_registry_ethereum.json"
        ))
        .unwrap();
        let markets = fixture["multi_quotes"].as_array().unwrap();
        assert_eq!(markets.len(), 4);
        let mut rounding = 0;
        for x in markets {
            let source: Address = x["source"].as_str().unwrap().parse().unwrap();
            let target: Address = x["target"].as_str().unwrap().parse().unwrap();
            let raw_pair = if source < target {
                [source, target]
            } else {
                [target, source]
            };
            let weth: Address = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
                .parse()
                .unwrap();
            let (tokens, native_side) = carbon::normalized(raw_pair, Some(weth)).unwrap();
            let s = CarbonState {
                controller: fixture["samples"]
                    .as_object()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()["address"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
                tokens,
                native_side,
                fee_ppm: 0,
                strategies: vec![],
            };
            let actions: Vec<_> = x["actions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| Action {
                    id: a[0].as_str().unwrap().parse().unwrap(),
                    amount: a[1].as_str().unwrap().parse().unwrap(),
                })
                .collect();
            let by_target = x["by_target"].as_bool().unwrap();
            let n = actions.iter().map(|a| a.amount).sum();
            let result: Bytes = serde_json::from_value(x["aggregate"]["result"].clone()).unwrap();
            let actual = u128::try_from(U256::from_be_slice(&result)).unwrap();
            let input = if source == carbon::NATIVE {
                weth
            } else {
                source
            };
            let mut p = Plan {
                actions,
                by_target,
                fee: 0,
                spent: if by_target { actual } else { n },
                received: if by_target { n } else { actual },
            };
            assert_eq!(
                payload(&s, input, &p).unwrap(),
                serde_json::from_value::<Bytes>(x["aggregate"]["data"].clone()).unwrap()
            );
            assert_eq!(decoded(&p, &result), Some(actual));
            if by_target {
                p.spent += 1;
            } else {
                p.received += 1;
            }
            assert_eq!(decoded(&p, &result), None);
            let sum = x["individual"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["result"].as_str().unwrap().parse::<U256>().unwrap())
                .sum::<U256>();
            rounding += usize::from(sum != U256::from(actual));
        }
        assert!(rounding > 0);
    }
}
