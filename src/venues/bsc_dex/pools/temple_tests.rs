//! Raw getters at the pinned Ethereum block. Both projections have one physical pair and the same rounding boundaries.
use super::*;
use crate::core::types::PoolRef;

#[test]
fn exact_amount_selects_a_physical_cash_asset_and_empty_dai_keeps_the_amm() {
    let j: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/temple_registry_ethereum.json"
    ))
    .unwrap();
    let addr = |s: &str| j[s].as_str().unwrap().parse::<Address>().unwrap();
    let n = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let two = |s: &str| [n(&j[s][0]), n(&j[s][1])];
    let s = TempleState {
        pair: addr("pair"),
        router: addr("router"),
        temple: addr("temple"),
        stable: addr("stable"),
        defend: addr("defend"),
        reserves: two("reserves"),
        balances: two("balances"),
        ratio: Some(two("ratio")),
        defense_balance: n(&j["defense_balance"]),
        amm_authorized: true,
    };
    let owner = j["owners"][1].as_str().unwrap();
    assert_eq!(j["role_controls"][owner]["threshold"], 2);
    assert_eq!(j["role_controls"][owner]["members"], 4);
    let token_pair = &j["token_pair"];
    assert!(s.defense_balance.is_zero());
    assert_eq!(
        token_pair[s.defend.to_string().to_lowercase()]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap(),
        Address::ZERO
    );
    assert_eq!(
        token_pair[s.stable.to_string().to_lowercase()]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap(),
        s.pair
    );
    let edge = |cash| {
        let (a, b) = super::super::discover::sorted(s.temple, cash);
        PoolRef::Multi {
            contract: s.pair,
            token0: a,
            token1: b,
        }
    };
    assert_ne!(edge(s.stable), edge(s.defend));
    assert_eq!(edge(s.stable).site(), edge(s.defend).site());
    let mut equal = 0;
    let mut defended = 0;
    let bounds = j["boundaries"].as_array().unwrap();
    let calls = j["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["method"] == "swapExactTempleForStableQuote(address,uint256)")
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), bounds.len());
    for (observation, call) in bounds.iter().zip(calls) {
        let data: Bytes = serde_json::from_value(call["data"].clone()).unwrap();
        let q = ITemple::swapExactTempleForStableQuoteCall::abi_decode_validate(&data).unwrap();
        assert_eq!(q.pair, s.pair);
        assert_eq!(q.amountIn, n(&observation["input"]));
        let bytes: Bytes = serde_json::from_value(call["result"].clone()).unwrap();
        let decoded =
            ITemple::swapExactTempleForStableQuoteCall::abi_decode_returns_validate(&bytes)
                .unwrap();
        assert_eq!(
            decoded.priceBelowIV,
            observation["defence"].as_bool().unwrap()
        );
        assert_eq!(decoded.amountOut, n(&observation["out"]));
        if n(&observation["amm"]) == n(&observation["iv"]) {
            assert!(!decoded.priceBelowIV);
            equal += 1;
        }
        assert_eq!(
            checked_payout(&s, s.temple, s.stable, q.amountIn, &bytes),
            (!decoded.priceBelowIV).then_some(decoded.amountOut)
        );
        assert_eq!(
            checked_payout(&s, s.temple, s.defend, q.amountIn, &bytes),
            None
        );
        defended += usize::from(decoded.priceBelowIV);
    }
    assert!(equal >= 2 && defended > 0);
}
