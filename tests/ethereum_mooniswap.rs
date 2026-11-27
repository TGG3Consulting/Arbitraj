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
