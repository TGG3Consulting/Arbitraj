//! Полный реестр фабрики DerpDEX Base на архивном блоке и фактическая глубина по направлению и размеру.
use std::collections::BTreeSet;

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::Value;

fn directory() -> Value {
    serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/derpdex_directory.json"
    ))
    .unwrap()
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn deployment_directory_covers_factory_creations_and_all_configured_fee_tiers() {
    let dir = directory();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let config = &profiles
        .iter()
        .find(|p| p["network"] == "base" && p["id"] == "derpdex-base")
        .unwrap()["settings"];
    let archive: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/derpdex_base.json"
    ))
    .unwrap();
    let init: B256 = archive["provenance"]["pool"]["actual"]["init_hash"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let factory = addr(&dir["factory"]);
    assert_eq!(addr(&config["dex.factory"]), factory);
    let tiers: BTreeSet<_> = config["dex.v3_fee_tiers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        tiers,
        dir["tiers"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect()
    );
    let mut deployed = BTreeSet::new();
    let mut events = BTreeSet::new();
    for row in dir["pools"].as_array().unwrap() {
        let fee = row["fee"].as_u64().unwrap();
        assert_eq!(dir["tiers"][fee.to_string()], row["spacing"]);
        let a = addr(&row["tokens"][0]);
        let b = addr(&row["tokens"][1]);
        assert!(a < b);
        let salt = keccak256((a, b, U256::from(fee)).abi_encode());
        assert_eq!(factory.create2(salt, init), addr(&row["pool"]));
        assert!(deployed.insert(addr(&row["pool"])));
        assert!(events.insert((row["tx"].as_str().unwrap(), row["log"].as_u64().unwrap())));
    }
    assert_eq!(deployed.len(), 13);
    assert_eq!(dir["nonce"].as_u64().unwrap(), 1 + deployed.len() as u64);
    assert_eq!(
        dir["storage_slots"].as_u64().unwrap(),
        (2 * deployed.len() + 1 + tiers.len()) as u64
    );
    let indexed: BTreeSet<_> = dir["indexed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| addr(&p["pool"]))
        .collect();
    assert_eq!(indexed.len(), 6);
    assert!(indexed.is_subset(&deployed));
}

fn raw(row: &Value, key: &str) -> U256 {
    row[key].as_str().unwrap().parse().unwrap()
}

fn check_result(reply: &Value, requested: U256) {
    use arb::venues::bsc_dex::quote::virtual_quoter;
    if reply["status"] == "0x0" {
        assert!(reply["reason"].is_object());
        return;
    }
    assert_eq!(reply["status"], "0x1");
    let used = raw(reply, "used");
    let out = raw(reply, "out");
    assert!(used <= requested);
    let abi = hex::decode(
        reply["returnData"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
    )
    .unwrap();
    assert_eq!(abi.len(), 64);
    assert_eq!(U256::from_be_slice(&abi[..32]), used);
    assert_eq!(U256::from_be_slice(&abi[32..]), out);
    let answer = virtual_quoter::decode(&abi, requested).unwrap();
    if used < requested {
        assert!(answer.is_err());
    } else {
        assert_eq!(answer.unwrap(), out);
    }
    assert_eq!(reply["full"], used == requested && out > U256::ZERO);
}

fn check_wallet(case: &Value) -> bool {
    if !case["sold"].is_string() {
        assert!(case["reason"].is_string());
        return false;
    }
    let buy = &case["buy_quote"];
    let sell = &case["sell_quote"];
    assert_eq!(buy["requested"], case["input"]);
    assert_eq!(buy["out"], case["bought"]);
    assert_eq!(sell["requested"], case["bought"]);
    assert_eq!(sell["out"], case["sold"]);
    for quote in [buy, sell] {
        assert!(raw(quote, "used") > U256::ZERO && raw(quote, "used") <= raw(quote, "requested"));
        assert!(raw(quote, "out") > U256::ZERO);
    }
    assert_eq!(
        raw(case, "cash_after"),
        raw(case, "cash_before") - raw(buy, "used") + raw(sell, "out")
    );
    let coin_before = case["coin_before"]
        .as_str()
        .map_or(U256::ZERO, |n| n.parse().unwrap());
    assert_eq!(
        raw(case, "after_coin_owner"),
        coin_before + raw(buy, "out") - raw(sell, "used")
    );
    for k in ["after_coin_executor", "after_cash_executor"] {
        assert_eq!(raw(case, k), U256::ZERO);
    }
    assert_eq!(case["min_out_plus_one_buy_reverted"], true);
    assert_eq!(case["min_out_plus_one_sell_reverted"], true);
    true
}

#[test]
fn all_directions_and_sizes_retain_their_own_quotes_funding_and_balance_deltas() {
    use arb::venues::bsc_dex::quote::virtual_quoter;
    let dir = directory();
    let archive: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/derpdex_markets.json"
    ))
    .unwrap();
    assert_eq!(archive["selector"], dir["selector"]);
    assert_eq!(archive["state_root"], dir["state_root"]);
    assert_eq!(
        archive["virtual_quoter_keccak"],
        format!("{:#x}", keccak256(virtual_quoter::runtime().unwrap()))
    );
    let pools = archive["markets"].as_array().unwrap();
    let all = dir["pools"].as_array().unwrap();
    assert_eq!(pools.len(), all.len());
    let mut passed = BTreeSet::new();
    let (mut funded, mut widened, mut existing_holder) = (0, 0, 0);
    let (mut reverted, mut recovered_quotes, mut zero_out, mut charged_zero) = (0, 0, 0, 0);
    for (market, deployment) in pools.iter().zip(all) {
        assert_eq!(market["pool"], deployment["pool"]);
        assert_eq!(market["tokens"], deployment["tokens"]);
        let quotes = market["quotes"].as_array().unwrap();
        for side in [0, 1] {
            let mut distinct = BTreeSet::new();
            for q in quotes.iter().filter(|q| q["side"] == side) {
                let n = raw(q, "requested");
                assert!(distinct.insert(n));
                check_result(q, n);
                if q["out"] == "0" {
                    zero_out += 1;
                    charged_zero += usize::from(q["used"] == q["requested"]);
                }
                if q["status"] == "0x0" {
                    reverted += 1;
                    let wider = &q["larger_gas"];
                    recovered_quotes += usize::from(wider["full"] == true);
                    assert!(
                        wider["gas_limit"].as_u64().unwrap() > q["gas_limit"].as_u64().unwrap()
                    );
                    check_result(wider, n);
                }
            }
            assert_eq!(distinct.len(), 4);
        }
        let wallet = market["wallet"].as_array().unwrap();
        assert_eq!(wallet.len(), 4);
        for case in wallet {
            if check_wallet(case) {
                funded += 1;
                passed.insert(addr(&market["pool"]));
            }
        }
        for case in market["larger_gas_wallet"].as_array().into_iter().flatten() {
            if check_wallet(case) {
                widened += 1;
            }
        }
        if let Some(cases) = market["existing_holder"]["wallet"].as_array() {
            assert_eq!(market["existing_holder"]["creation_tx"], market["tx"]);
            for c in cases {
                if check_wallet(c) {
                    assert!(!passed.contains(&addr(&market["pool"])));
                    existing_holder += 1;
                    let side = c["side"].as_u64().unwrap() as usize;
                    assert_eq!(
                        market["existing_holder"]["balances"][side],
                        c["cash_before"]
                    );
                    assert_eq!(
                        market["existing_holder"]["balances"][1 - side],
                        c["coin_before"]
                    );
                }
            }
        }
    }
    assert_eq!(
        (funded, widened, existing_holder, passed.len()),
        (27, 3, 4, 8)
    );
    assert_eq!(
        (reverted, recovered_quotes, zero_out, charged_zero),
        (16, 8, 28, 4)
    );
    assert!(
        dir["indexed"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| passed.contains(&addr(&p["pool"])))
    );
}
