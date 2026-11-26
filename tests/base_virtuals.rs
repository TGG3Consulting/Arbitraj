//! Сверяет покупку и продажу с расходом и зачислением реального владельца в архиве обеих кривых.
use alloy_primitives::{Address, U256};
use arb::{core::types::VirtualsState, venues::bsc_dex::math::virtuals};
use flate2::read::GzDecoder;
use serde_json::Value;

fn num(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn result<'a>(leg: &'a Value, side: &str, key: &str) -> &'a Value {
    &leg["results"][format!("{side}{key}")]
}
fn words(leg: &Value, side: &str, key: &str) -> Vec<U256> {
    let data = result(leg, side, key)["returnData"]
        .as_str()
        .unwrap()
        .strip_prefix("0x")
        .unwrap();
    data.as_bytes()
        .chunks_exact(64)
        .map(|v| U256::from_str_radix(std::str::from_utf8(v).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn forward_inverse_and_actual_wallet_agree_on_both_curves() {
    let captures: &[&[u8]] = &[
        include_bytes!("data/first_run_dex_2026-11-26/virtuals-base-protected.json.gz"),
        include_bytes!("data/first_run_dex_2026-11-26/virtuals-unicorn-base-protected.json.gz"),
    ];
    for raw in captures {
        let snapshot: Value = serde_json::from_reader(GzDecoder::new(*raw)).unwrap();
        let registry = &snapshot["registry"];
        let vs = &registry["samples"][0];
        let tokens = &snapshot["tokens"];
        let agent = tokens[1].as_str().unwrap().parse::<Address>().unwrap();
        let asset = tokens[0].as_str().unwrap().parse::<Address>().unwrap();
        let cfg: Vec<Value> =
            serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
        let settings = &cfg
            .iter()
            .find(|r| r["id"] == snapshot["id"] && r["chain_id"] == 8453)
            .unwrap()["settings"];
        assert_eq!(
            settings["dex.factory"],
            registry["roles"]["factory"]["address"]
        );
        assert_eq!(settings["dex.quoter"], snapshot["router"]);
        for case in snapshot["cases"].as_array().unwrap() {
            for side in ["buy", "sell"] {
                let buy = side == "buy";
                let leg = &case["module"][side];
                let amount = num(&leg["amount"]);
                let net = num(&leg["net"]);
                let native = &case["native"][side];
                assert_eq!(leg["net"], native["net"]);
                let rx = words(leg, side, "reserves");
                let back = words(leg, side, "backing")[0];
                let mut state = VirtualsState {
                    agent,
                    asset,
                    reserves: [rx[0], rx[1]],
                    backing: [num(&vs["balances"][0]), num(&vs["balances"][1])],
                    invariant: words(leg, side, "invariant")[0],
                    graduation: None,
                    buy_tax: 1,
                    sell_tax: 1,
                    launch_tax: 0,
                    external_swap: false,
                };
                state.backing[usize::from(!buy)] = back;
                let input = if buy { asset } else { agent };
                let swap = virtuals::quote(&state, input, true, amount).unwrap();
                assert_eq!(swap.amount_out, net);
                assert_eq!(
                    words(leg, side, "before_in")[0] - words(leg, side, "after_in")[0],
                    amount
                );
                assert_eq!(
                    words(leg, side, "after_out")[0] - words(leg, side, "before_out")[0],
                    net
                );
                assert_eq!(words(leg, side, "exact")[0], net);
                for key in ["strict", "expired"] {
                    assert_eq!(result(leg, side, key)["status"], "0x0");
                }
                for key in ["held_in", "held_out", "module_allow", "wallet_allow"] {
                    assert_eq!(words(leg, side, key)[0], U256::ZERO);
                }
                let inverse = virtuals::quote(&state, input, false, net).unwrap();
                assert!(inverse.amount_in <= amount && inverse.amount_out >= net);
                assert!(
                    virtuals::quote(&state, input, true, inverse.amount_in - U256::ONE)
                        .is_none_or(|x| x.amount_out < net)
                );
                if buy {
                    state.graduation = Some(rx[0] - net);
                    assert!(virtuals::quote(&state, asset, true, amount).is_none());
                } else {
                    state.backing[1] = num(&leg["gross"]) - U256::ONE;
                    assert!(virtuals::quote(&state, agent, true, amount).is_none());
                }
            }
        }
    }
}
#[test]
fn independent_tax_rounding_finds_the_first_sufficient_input() {
    let book = VirtualsState {
        agent: Address::with_last_byte(1),
        asset: Address::with_last_byte(2),
        reserves: [U256::from(3000); 2],
        backing: [U256::from(3000); 2],
        invariant: U256::from(9_000_000),
        graduation: None,
        buy_tax: 13,
        sell_tax: 5,
        launch_tax: 21,
        external_swap: false,
    };
    for want in 1..200u64 {
        let want = U256::from(want);
        let direct = virtuals::quote(&book, book.asset, false, want).unwrap();
        let first = (1..500u64)
            .find(|n| {
                virtuals::quote(&book, book.asset, true, U256::from(*n))
                    .is_some_and(|x| x.amount_out >= want)
            })
            .unwrap();
        assert_eq!(direct.amount_in, U256::from(first));
    }
}
