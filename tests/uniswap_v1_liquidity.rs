//! One V1 directory factory can expose many ERC20/native markets and composite ERC20 pairs.
#[allow(dead_code)]
mod native_common;
use alloy_primitives::Address;
use arb::core::types::{
    PoolRef, PoolState,
    uniswap_v1::{UniswapV1State, V1Exchange},
};

#[test]
fn single_and_double_exchange_routes_conflict_only_at_shared_physical_reserves() {
    let v: serde_json::Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/uniswap_v1_ethereum_prices.json"
    ))
    .unwrap();
    let factory: Address = v["factory"].as_str().unwrap().parse().unwrap();
    let wrapped: Address = v["wrapped"].as_str().unwrap().parse().unwrap();
    let markets = v["markets"].as_object().unwrap();
    let exchange = |key: &str| {
        let token: Address = key.parse().unwrap();
        let x = &markets[key];
        V1Exchange {
            address: x["exchange"].as_str().unwrap().parse().unwrap(),
            token,
            eth: x["eth"].as_str().unwrap().parse().unwrap(),
            erc20: x["erc20"].as_str().unwrap().parse().unwrap(),
        }
    };
    let tokens: Vec<_> = markets.keys().map(|k| k.as_str()).collect();
    let make = |a: Option<&str>, b: Option<&str>| {
        let mut e = native_common::entry("uniswap_v1", None);
        let tokens = [a, b].map(|t| t.map(|k| k.parse().unwrap()).unwrap_or(wrapped));
        let markets = [a.map(exchange), b.map(exchange)];
        e.pool.reference = if let [Some(_), Some(_)] = markets {
            PoolRef::Multi {
                contract: factory,
                token0: tokens[0],
                token1: tokens[1],
            }
        } else {
            PoolRef::Contract(markets.iter().flatten().next().unwrap().address)
        };
        e.pool.state = PoolState::UniswapV1(Box::new(UniswapV1State {
            factory,
            tokens,
            markets,
        }));
        e
    };
    let a = make(Some(tokens[0]), Some(tokens[1]));
    let b = make(Some(tokens[1]), Some(tokens[2]));
    let c = make(Some(tokens[2]), Some(tokens[3]));
    let s = make(Some(tokens[1]), None);
    assert!(a.shares_liquidity(&b));
    assert!(s.shares_liquidity(&a) && a.shares_liquidity(&s));
    assert!(!a.shares_liquidity(&c));
    assert!(!s.shares_liquidity(&c));
}
