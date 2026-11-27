//! Сумма для графа, минимальный вход и чистая выплата обеих касс на одном блоке.
use alloy_primitives::{Address, U256};
use arb::core::types::defiplaza::{DefiPlazaCurve, DefiPlazaState};
use serde_json::Value;
fn read(raw: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(raw)).unwrap()
}
fn raw(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
#[test]
fn funded_paths_keep_raw_amounts_and_cannot_spend_another_pairs_reserves() {
    for (native, protected, stable) in [
        (
            include_bytes!("data/first_run_dex_2026-11-26/defiplaza_multi_ethereum.json.gz")
                .as_slice(),
            include_bytes!(
                "data/first_run_dex_2026-11-26/defiplaza_multi_protected_ethereum.json.gz"
            )
            .as_slice(),
            false,
        ),
        (
            include_bytes!("data/first_run_dex_2026-11-26/defiplaza_stable_ethereum.json.gz")
                .as_slice(),
            include_bytes!(
                "data/first_run_dex_2026-11-26/defiplaza_stable_protected_ethereum.json.gz"
            )
            .as_slice(),
            true,
        ),
    ] {
        let book = read(native);
        let guarded = read(protected);
        assert_eq!(book["selector"], guarded["selector"]);
        let inventory = &book["inventory"];
        let assets = inventory["assets"].as_array().unwrap();
        let w: Address = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
            .parse()
            .unwrap();
        for leg in book["trials"].as_array().unwrap() {
            let ids = [&leg["input"], &leg["output"]];
            let physical = ids.map(|v| v.as_str().unwrap().parse::<Address>().unwrap());
            let tokens = physical.map(|a| if a.is_zero() { w } else { a });
            let positions = ids.map(|a| assets.iter().position(|v| v == a).unwrap());
            let balances = ids.map(|v| raw(&inventory["balances"][v.as_str().unwrap()]));
            let (factors, reserves, curve) = if stable {
                let read_arr = |name: &str| {
                    positions
                        .map(|j| U256::from(inventory["registration"][name][j].as_u64().unwrap()))
                };
                (
                    read_arr("denorm"),
                    read_arr("reserves"),
                    DefiPlazaCurve::Stable {
                        fee_bps: inventory["config"][1].as_u64().unwrap().try_into().unwrap(),
                        delta: inventory["config"][6].as_u64().unwrap(),
                    },
                )
            } else {
                (
                    [U256::ONE; 2],
                    balances,
                    DefiPlazaCurve::Multi {
                        one_minus_fee: inventory["config"][1].as_u64().unwrap(),
                    },
                )
            };
            let state = DefiPlazaState {
                pool: book["pool"].as_str().unwrap().parse().unwrap(),
                module_hash: guarded["runtime"].as_str().unwrap().parse().unwrap(),
                module: guarded["module"].as_str().unwrap().parse().unwrap(),
                tokens,
                physical,
                balances,
                reserves,
                factors,
                curve,
            };
            let spent = raw(&leg["raw"]);
            let net = raw(&leg["exact"]["received"]);
            let fill = state.swap(tokens[0], true, spent).unwrap();
            assert_eq!((fill.amount_in, fill.amount_out), (spent, net));
            let target = state.swap(tokens[0], false, net).unwrap();
            assert!(target.amount_in <= spent);
            assert!(
                state
                    .swap(tokens[0], true, target.amount_in - U256::ONE)
                    .is_none_or(|f| f.amount_out < net)
            );
            let next = state.after(tokens[0], spent, net).unwrap();
            assert_eq!(
                next.balances,
                [
                    raw(&leg["exact"]["after"][2]),
                    raw(&leg["exact"]["after"][3])
                ]
            );
            assert!(
                state
                    .swap(tokens[0], false, balances[1] + U256::ONE)
                    .is_none()
            );
            let exact = guarded["fills"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|v| {
                    v["mode"] == "v5"
                        && v["input"].as_str().and_then(|t| t.parse::<Address>().ok())
                            == Some(tokens[0])
                        && v["output"].as_str().and_then(|t| t.parse::<Address>().ok())
                            == Some(tokens[1])
                        && raw(&v["spent"]) == spent
                })
                .collect::<Vec<_>>();
            assert_eq!(exact.len(), 1);
            assert_eq!(raw(&exact[0]["received"]), net);
        }
    }
}
