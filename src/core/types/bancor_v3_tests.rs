use super::*;
use serde_json::Value;
use std::io::Read;
const DIR: &str =
    include_str!("../../../tests/data/first_run_dex_2026-11-26/bancor_v3_liquidity.json");
fn num(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn key(a: Address) -> String {
    format!("{a:#x}")
}
fn archive() -> (Value, Value) {
    let raw =
        include_bytes!("../../../tests/data/first_run_dex_2026-11-26/bancor_v3_ethereum.json.gz");
    let mut text = Vec::new();
    flate2::read::GzDecoder::new(raw.as_slice())
        .read_to_end(&mut text)
        .unwrap();
    (
        serde_json::from_slice(&text).unwrap(),
        serde_json::from_str(DIR).unwrap(),
    )
}
fn state(d: &Value, l: &Value, source: Address, target: Address) -> BancorState {
    let bnt = addr(&d["bnt"]);
    let roles = &d["roles"];
    let wrapped = alloy_primitives::address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
    let native = Address::repeat_byte(0xee);
    let norm = |t| if t == native { wrapped } else { t };
    let mut physical = [source, target];
    physical.sort_by_key(|t| norm(*t));
    let markets = physical.map(|t| {
        if t == bnt {
            return None;
        }
        let p = &d["pools"][key(t)];
        let m = &l["pools"][key(t)];
        Some(BancorMarket {
            token: t,
            collection: addr(&p["collection"]),
            base: num(&p["base"]),
            bnt: num(&p["bnt"]),
            staked: num(&m["staked"]),
            fee_ppm: p["fee"].as_u64().unwrap() as u32,
            network_fee_ppm: m["network_fee"].as_u64().unwrap() as u32,
            enabled: p["enabled"].as_bool().unwrap(),
        })
    });
    BancorState {
        network: addr(&roles["BancorNetwork"]["address"]),
        info: addr(&roles["BancorNetworkInfo"]["address"]),
        vault: addr(&l["vault"]),
        bnt,
        bnt_decimals: 18,
        wrapped,
        tokens: physical.map(norm),
        physical,
        markets,
        vault_balances: physical.map(|t| num(&l["balances"][key(t)])),
    }
}
fn param(s: &BancorState, t: Address) -> Address {
    s.tokens[s.physical.iter().position(|a| *a == t).unwrap()]
}
#[test]
fn all_official_quotes_and_minimal_forward_budgets() {
    let (d, l) = archive();
    assert_eq!(d["selector"], l["selector"]);
    assert_eq!(d["pools"].as_object().unwrap().len(), 148);
    let q = d["quotes"].as_array().unwrap();
    assert!(q.len() >= 354);
    let mut strict = 0;
    for row in q {
        let a = addr(&row["source"]);
        let b = addr(&row["target"]);
        let s = state(&d, &l, a, b);
        let input = param(&s, a);
        let exact = row["exact_input"].as_bool().unwrap();
        let n = num(&row["amount"]);
        let value = num(&row["result"]);
        let f = s.swap(input, exact, n, false).unwrap();
        assert_eq!(if exact { f.received } else { f.spent }, value);
        if !exact {
            let g = s.input_for_forward(input, n, false).unwrap();
            assert!(g.received >= n);
            assert!(
                s.swap(input, true, g.spent - U256::ONE, false)
                    .is_none_or(|prior| prior.received < n)
            );
            if f.spent < g.spent {
                strict += 1;
            }
        }
    }
    assert!(strict > 0);
}
#[test]
fn paid_amounts_and_collection_network_fee_update_match_every_event() {
    let (d, l) = archive();
    let rows = d["cases"].as_array().unwrap();
    assert!(rows.len() >= 38);
    let topic = alloy_primitives::keccak256(
        "TradingLiquidityUpdated(bytes32,address,address,uint256,uint256)",
    );
    for row in rows {
        let a = addr(&row["source"]);
        let b = addr(&row["target"]);
        let s = state(&d, &l, a, b);
        let mut next = s.clone();
        let result = s
            .trade(
                param(&s, a),
                row["exact_input"].as_bool().unwrap(),
                num(&row["amount"]),
                false,
                |i, [base, bnt, staked]| {
                    let m = next.markets[i].as_mut().unwrap();
                    m.base = base;
                    m.bnt = bnt;
                    m.staked = staked;
                },
            )
            .unwrap();
        assert_eq!(
            (result.spent, result.received),
            (num(&row["spent"]), num(&row["received"]))
        );
        let mut count = 0;
        for log in row["logs"].as_array().unwrap() {
            let topics = log["topics"].as_array().unwrap();
            if topics[0].as_str().unwrap() != format!("{topic:#x}") {
                continue;
            }
            let raw_addr =
                |i: usize| Address::from_word(topics[i].as_str().unwrap().parse().unwrap());
            let pool = raw_addr(2);
            let token = raw_addr(3);
            let before = s
                .markets
                .iter()
                .flatten()
                .find(|m| m.token == pool)
                .unwrap();
            let after = next
                .markets
                .iter()
                .flatten()
                .find(|m| m.token == pool)
                .unwrap();
            assert_eq!(addr(&log["address"]), before.collection);
            let (prev, post) = if token == s.bnt {
                (before.bnt, after.bnt)
            } else {
                assert_eq!(pool, token);
                (before.base, after.base)
            };
            let data: alloy_primitives::Bytes = log["data"].as_str().unwrap().parse().unwrap();
            assert_eq!(
                (prev, post),
                (
                    U256::from_be_slice(&data[..32]),
                    U256::from_be_slice(&data[32..])
                )
            );
            count += 1;
        }
        assert!(count >= 2);
    }
}

#[test]
fn vault_covers_exact_target_while_minimal_forward_input_overpays() {
    let (d, l) = archive();
    let native = Address::repeat_byte(0xee);
    let bnt = addr(&d["bnt"]);
    let mut s = state(&d, &l, native, bnt);
    let input = param(&s, native);
    let i = s.side(input).unwrap();
    let m = s.markets[i].as_mut().unwrap();
    m.base = U256::from(10);
    m.bnt = U256::from(100);
    m.fee_ppm = 0;
    m.network_fee_ppm = 0;
    s.vault_balances[1 - i] = U256::from(51);
    let desired = U256::from(51);
    let paid = s.swap(input, false, desired, false).unwrap();
    assert_eq!(paid.spent, U256::from(10));
    assert_eq!(
        s.swap(input, true, paid.spent, false).unwrap().received,
        U256::from(50)
    );
    assert!(s.input_for_forward(input, desired, false).is_none());
    s.vault_balances[1 - i] = U256::from(52);
    let forward = s.input_for_forward(input, desired, false).unwrap();
    assert_eq!(
        (forward.spent, forward.received),
        (U256::from(11), U256::from(52))
    );
}

#[test]
fn v5_step_matches_physical_directory_and_normalized_input_for_all_paid_pairs() {
    let (d, l) = archive();
    let native = Address::repeat_byte(0xee);
    let mut forms = std::collections::BTreeSet::new();
    for row in d["cases"].as_array().unwrap() {
        let a = addr(&row["source"]);
        let b = addr(&row["target"]);
        let s = state(&d, &l, a, b);
        let mut tokens = [a, b];
        tokens.sort_unstable();
        let make = |pair: [Address; 2]| crate::core::types::PoolRef::Multi {
            contract: s.network, token0: pair[0], token1: pair[1],
        };
        let form = s.v5_form(&make(tokens),param(&s,a),native).unwrap();
        assert_eq!(form,u8::from(a == native) + 2*u8::from(b == native));
        forms.insert(form);
        if tokens.contains(&native) {
            assert!(s.v5_form(&make(s.tokens),param(&s,a),native).is_none());
        }
    }
    assert_eq!(forms.into_iter().collect::<Vec<_>>(),vec![0,1,2]);
}
