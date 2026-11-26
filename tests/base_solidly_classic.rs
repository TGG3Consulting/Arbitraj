use alloy_primitives::{Address, B256, U256, keccak256};
use arb::{
    core::types::{FeeRule, SolidlyFee},
    venues::bsc_dex::math::{solidly_fee, solidly_v1_math, v2},
};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::{fs::File, path::Path};

fn archive(kind: &str, id: &str) -> Value {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/first_run_dex_2026-11-26");
    serde_json::from_reader(GzDecoder::new(
        File::open(dir.join(format!("base_classic_{kind}_{id}.json.gz"))).unwrap(),
    ))
    .unwrap()
}
fn uint(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn reserves(reply: &Value) -> [U256; 2] {
    let h = reply["returnData"]
        .as_str()
        .unwrap()
        .strip_prefix("0x")
        .unwrap();
    [
        U256::from_str_radix(&h[..64], 16).unwrap(),
        U256::from_str_radix(&h[64..128], 16).unwrap(),
    ]
}
fn payout(m: &Value, rs: [U256; 2], i: usize, dx: U256, fee: &SolidlyFee) -> U256 {
    let net = solidly_fee::net_in(dx, fee).unwrap();
    if m["stable"] == true {
        solidly_v1_math::stable_amount_out(
            net,
            rs[i],
            rs[1 - i],
            uint(&m["scales"][i]),
            uint(&m["scales"][1 - i]),
            U256::ZERO,
        )
        .unwrap()
    } else {
        v2::amount_out(net, rs[i], rs[1 - i], (U256::ONE, U256::ONE)).unwrap()
    }
}

#[test]
fn factory_ownership_fee_and_both_wallet_directions_match_the_v1_curve() {
    let cfg: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let mut pools = 0;
    let mut fills = 0;
    let mut fresh_fills = 0;
    for id in [
        "archly-base",
        "archly-v1-base",
        "baso-finance",
        "equalizer-base",
        "hydrex-classic",
        "infusion",
        "velocimeter-base",
    ] {
        let dir = archive("registry", id);
        let wallets = archive("wallets", id);
        let profile = &cfg
            .iter()
            .find(|v| v["id"] == id && v["chain_id"] == 8453)
            .unwrap()["settings"];
        assert_eq!(profile["dex.kind"], "solidly_v1");
        assert_eq!(profile["dex.factory"], dir["factory"]["address"]);
        assert_eq!(
            profile["dex.factory_runtime_hash"],
            dir["factory"]["runtime_hash"]
        );
        assert_eq!(profile["dex.pool_code_hashes"][0], dir["init"]);
        assert_eq!(profile["dex.solidly_fee_source"], dir["fee_source"]);
        let denominator = dir["denominator"].as_str().unwrap().parse::<u64>().unwrap();
        assert_eq!(profile["dex.solidly_fee_denominator"], denominator);
        assert_eq!(wallets["selector"], dir["selector"]);
        assert_eq!(wallets["state_root"], dir["state_root"]);
        if let Some(imp) = dir["factory"].get("implementation") {
            assert_eq!(profile["dex.proxy_implementations"][0], imp["address"]);
            assert_eq!(profile["dex.proxy_runtime_hashes"][0], imp["runtime_hash"]);
        }
        let rows = dir["markets"].as_array().unwrap();
        pools += rows.len();
        let factory = profile["dex.factory"]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap();
        let init = dir["init"].as_str().unwrap().parse::<B256>().unwrap();
        for row in rows {
            let mut salt = Vec::with_capacity(41);
            for t in row["tokens"].as_array().unwrap() {
                salt.extend_from_slice(t.as_str().unwrap().parse::<Address>().unwrap().as_slice());
            }
            salt.push(u8::from(row["stable"].as_bool().unwrap()));
            assert_eq!(
                factory.create2(keccak256(&salt), init)
                    == row["pool"].as_str().unwrap().parse::<Address>().unwrap(),
                row["current_init"].as_bool().unwrap()
            );
        }
        let measured = wallets["markets"].as_array().unwrap();
        for stable in [false, true] {
            assert!(
                measured.iter().any(|m| m["stable"] == stable),
                "{id} stable={stable}"
            );
        }
        for market in measured {
            let m = rows.iter().find(|r| r["pool"] == market["pool"]).unwrap();
            assert_eq!(m["current_init"], true);
            assert_eq!(m["balances"], m["reserves"]);
            let i = usize::from(m["tokens"][0] != market["token_in"]);
            let fee = SolidlyFee {
                value: m["fee"].as_str().unwrap().parse().unwrap(),
                rule: FeeRule::Numerator { denominator },
            };
            for trial in market["cases"].as_array().unwrap() {
                let original = [uint(&m["reserves"][0]), uint(&m["reserves"][1])];
                let bought = payout(m, original, i, uint(&trial["amount"]), &fee);
                assert_eq!(bought, uint(&trial["bought"]));
                let after = reserves(&trial["results"]["after_reserves"]);
                assert_eq!(payout(m, after, 1 - i, bought, &fee), uint(&trial["sold"]));
                if id == "archly-v1-base" && m["stable"] == true {
                    let independent = &trial["unconnected_wallet"];
                    assert_ne!(independent["owner"], trial["owner"]);
                    assert_eq!(independent["amount"], trial["amount"]);
                    assert_eq!(independent["bought"], trial["bought"]);
                    assert_eq!(independent["sold"], trial["sold"]);
                    for side in ["buy", "sell"] {
                        assert_eq!(independent["results"][side]["status"], "0x1");
                        assert_eq!(
                            independent["results"][format!("above_{side}")]["status"],
                            "0x0"
                        );
                        fresh_fills += 1;
                    }
                }
                for side in ["buy", "sell"] {
                    assert_eq!(trial["results"][side]["status"], "0x1");
                    assert_eq!(trial["results"][format!("above_{side}")]["status"], "0x0");
                    fills += 1;
                }
            }
        }
    }
    assert_eq!(pools, 1169);
    assert_eq!((fills, fresh_fills), (56, 4));
}

#[test]
fn equalizer_preserves_sub_basis_point_precision() {
    let fee = SolidlyFee {
        value: 166666666666667,
        rule: FeeRule::Numerator {
            denominator: 1_000_000_000_000_000_000,
        },
    };
    let input = U256::from(1_000_000_000_000_000_000u64);
    assert_eq!(
        input - solidly_fee::net_in(input, &fee).unwrap(),
        U256::from(166666666666667u64)
    );
}
