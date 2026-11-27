//! Проверка обеих физических касс и обеих сторон виртуальных остатков Mooniswap в одном блоке.
use alloy_primitives::{Address, U256};
use arb::core::types::MooniswapState;
use serde_json::Value;

fn n(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn virtuals(v: &Value) -> [(U256, u64); 2] {
    std::array::from_fn(|i| (n(&v[i][0]), v[i][1].as_str().unwrap().parse().unwrap()))
}
fn read(bytes: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
}
#[test]
fn virtual_tariffs_and_wallet_payments_follow_originals_in_both_directions() {
    for (bytes, successor, expected) in [
        (
            include_bytes!("data/first_run_dex_2026-11-26/mooniswap_original_ethereum.json.gz")
                .as_slice(),
            false,
            28,
        ),
        (
            include_bytes!("data/first_run_dex_2026-11-26/mooniswap_successor_ethereum.json.gz")
                .as_slice(),
            true,
            24,
        ),
    ] {
        let tape = read(bytes);
        historical_decay(&tape, successor);
        let mut count = 0;
        for pool in tape["markets"].as_array().unwrap() {
            for t in pool["trials"].as_array().unwrap() {
                let Some(fill) = t.get("exact") else { continue };
                let input = t["input"].as_str().unwrap().parse::<Address>().unwrap();
                let output = t["output"].as_str().unwrap().parse::<Address>().unwrap();
                // Обсерватор записал виртуальные поля в порядке входа и выхода.
                let v = &t["frame_before"];
                let after = &t["frame_after"];
                let s = MooniswapState {
                    physical: [input, output],
                    tokens: [input, output],
                    balances: [n(&fill["before"][2]), n(&fill["before"][3])],
                    addition: virtuals(&v["addition"]),
                    removal: virtuals(&v["removal"]),
                    timestamp: n(&v["timestamp"]).to(),
                    decay: n(&v["decay"]).to(),
                    fee: n(&v["fee"]),
                    slippage_fee: n(&v["slippage_fee"]),
                    successor,
                };
                let amount = n(&t["amount"]);
                let net = n(&fill["received"]);
                let forward = s.swap(input, true, amount).unwrap();
                assert_eq!(
                    (forward.amount_in, forward.amount_out),
                    (n(&fill["spent"]), net)
                );
                let inverse = s.swap(input, false, net).unwrap();
                assert!(inverse.amount_in <= amount && inverse.amount_out >= net);
                assert!(
                    s.swap(input, true, inverse.amount_in - U256::ONE)
                        .is_none_or(|x| x.amount_out < net)
                );
                let next = s.after(input, amount, net).unwrap();
                assert_eq!(next.balances, [n(&fill["after"][2]), n(&fill["after"][3])]);
                assert_eq!(next.addition, virtuals(&after["addition"]));
                assert_eq!(next.removal, virtuals(&after["removal"]));
                assert_eq!(
                    (s.fee, s.slippage_fee, s.decay),
                    (
                        n(&after["fee"]),
                        n(&after["slippage_fee"]),
                        n(&after["decay"]).to()
                    )
                );
                // После полного периода старая виртуальная пара должны совпасть с физической.
                let mut delayed = next;
                delayed.timestamp += delayed.decay;
                assert_eq!(
                    delayed.reserves(input),
                    Some((delayed.balances[0], delayed.balances[1]))
                );
                count += 1;
            }
        }
        assert_eq!(count, expected);
    }
}

#[test]
fn first_funded_input_matches_exhaustive_small_reserves_at_both_sides_of_price_peak() {
    let x = Address::ZERO;
    let y = Address::repeat_byte(1);
    for fee in [0, 3_000_000_000_000_000u64] {
        for slip in [
            0,
            75_000_000_000_000_000u64,
            828_830_973_208_080_598,
            1_000_000_000_000_000_000,
        ] {
            let state = MooniswapState {
                physical: [x, y],
                tokens: [x, y],
                balances: [U256::from(39), U256::from(83)],
                addition: [(U256::ZERO, 0); 2],
                removal: [(U256::ZERO, 0); 2],
                timestamp: 1000,
                decay: 80,
                fee: U256::from(fee),
                slippage_fee: U256::from(slip),
                successor: true,
            };
            let outs: Vec<_> = (1u64..=512)
                .map(|q| {
                    state
                        .swap(x, true, U256::from(q))
                        .map_or(U256::ZERO, |f| f.amount_out)
                })
                .collect();
            for desired in 1u64..83 {
                let target = U256::from(desired);
                let first = outs
                    .iter()
                    .position(|out| *out >= target)
                    .map(|n| U256::from(n + 1));
                let actual = state
                    .swap(x, false, target)
                    .map(|f| f.amount_in)
                    .filter(|q| *q <= U256::from(512));
                assert_eq!(actual, first);
            }
        }
    }
}

fn historical_decay(tape: &Value, successor: bool) {
    let case = &tape["delay"];
    let samples = case["frames"].as_array().unwrap();
    assert_eq!(samples.len(), 4);
    let pair: [Address; 2] = [
        case["input"].as_str().unwrap().parse().unwrap(),
        case["output"].as_str().unwrap().parse().unwrap(),
    ];
    let first = &samples[0];
    let qty = n(&case["amount"]);
    let mut s = MooniswapState {
        physical: pair,
        tokens: pair,
        balances: [n(&first["balances"][0]), n(&first["balances"][1])],
        addition: virtuals(&first["addition"]),
        removal: virtuals(&first["removal"]),
        timestamp: first["timestamp"].as_u64().unwrap(),
        decay: n(&first["decay"]).to(),
        fee: n(&first["fee"]),
        slippage_fee: n(&first["slippage_fee"]),
        successor,
    };
    let original = s.timestamp;
    for row in samples {
        assert!(
            [
                "addition",
                "removal",
                "balances",
                "fee",
                "slippage_fee",
                "decay"
            ]
            .iter()
            .all(|k| row[*k] == first[*k])
        );
        s.timestamp = row["timestamp"].as_u64().unwrap();
        assert_eq!(
            s.swap(pair[0], true, qty).unwrap().amount_out,
            n(&row["quote"])
        );
    }
    assert!(s.timestamp >= original + s.decay);
    assert_ne!(samples[0]["quote"], samples[3]["quote"]);
}
