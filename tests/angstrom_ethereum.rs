//! Приёмка Angstrom L1 по двум физическим ключам V4 и каноническим движениям кошелька.
use alloy_primitives::{Address, B256, Signature, U256};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::fs::File;

fn snap(file: &str) -> Value {
    let f = File::open(format!(
        "tests/data/first_run_dex_2026-11-26/{file}.json.gz"
    ))
    .unwrap();
    serde_json::from_reader(GzDecoder::new(f)).unwrap()
}

const TRANSFER: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
const SWAP: &str = "0x40e9cecb9f5f1f1c5b9c97dec2917b7ee92e57ba5563708daca94dd84ad7112f";

fn transfer(logs: &[Value], token: &Value, from: &Value, to: &Value) -> u128 {
    let address = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap().into_word();
    logs.iter()
        .filter(|l| {
            l["address"] == *token
                && l["topics"][0] == TRANSFER
                && l["topics"][1].as_str().unwrap().parse::<B256>().unwrap() == address(from)
                && l["topics"][2].as_str().unwrap().parse::<B256>().unwrap() == address(to)
        })
        .map(|l| u128::from_str_radix(&l["data"].as_str().unwrap()[2..], 16).unwrap())
        .sum()
}

#[test]
fn signed_block_and_wallet_debits_match_the_paid_protocol_trade() {
    let r = snap("angstrom_ethereum_wallets");
    let d = snap("angstrom_ethereum_directory");
    let wire = r["wire_block"].as_u64().unwrap();
    let h = r["block"].as_u64().unwrap();
    let sig = r["source_data"]
        .as_str()
        .unwrap()
        .parse::<alloy_primitives::Bytes>()
        .unwrap();
    let signature = Signature::try_from(&sig[20..]).unwrap();
    let node = Address::from_slice(&sig[..20]);
    assert_ne!(wire, h);
    for (n, entry) in r["eip712"].as_object().unwrap() {
        let signer = signature
            .recover_address_from_prehash(&entry["digest"].as_str().unwrap().parse().unwrap())
            .unwrap();
        assert_eq!(signer == node, n == &h.to_string());
    }
    for row in r["controls"].as_array().unwrap() {
        for key in ["wrong_block", "empty", "unassisted_v5"] {
            assert_eq!(row[key]["status"], "0x0");
        }
        for key in ["public_unlock", "v5_after_unlock"] {
            assert_eq!(row[key]["status"], "0x1");
        }
    }
    let manager = Value::from("0x000000000004444c5dc75cb358380d2e3de08a90");
    let mut paid = 0;
    for a in r["payments"].as_array().unwrap() {
        let id = a["pool"].as_str().unwrap();
        let market = &d["directory"][id];
        assert_eq!(market["batch_fee"], 200);
        assert_eq!(market["unlocked_lp"], 250);
        assert_eq!(market["unlocked_protocol"], 150);
        for l in ["buy", "sell"].iter().filter_map(|k| a.get(*k)) {
            let x = &l["exact"];
            let logs = x["logs"].as_array().unwrap();
            let debit = x["spent"].as_str().unwrap().parse::<u128>().unwrap();
            let net = x["received"].as_str().unwrap().parse::<u128>().unwrap();
            assert_eq!(transfer(logs, &l["input"], &r["owner"], &manager), debit);
            assert_eq!(transfer(logs, &l["output"], &manager, &r["owner"]), net);
            let v: Vec<_> = logs
                .iter()
                .filter(|v| {
                    v["address"] == manager && v["topics"][0] == SWAP && v["topics"][1] == id
                })
                .collect();
            assert_eq!(v.len(), 1);
            let data = &v[0]["data"].as_str().unwrap()[2..];
            let out = usize::from(l["output"] != market["key"]["currency0"]);
            let gross = U256::from_str_radix(&data[64 * out..64 * (out + 1)], 16)
                .unwrap()
                .to::<u128>();
            assert_eq!(net, gross - (gross * 150 / 1_000_000));
            for k in ["above", "expired"] {
                assert_eq!(l[k]["status"], "0x0");
                assert_eq!(l[k]["received"], "0");
                assert_eq!(l[k]["spent"], "0");
            }
            paid += 1;
        }
    }
    assert_eq!(paid, 12);
}

#[test]
fn a_top_of_block_price_includes_the_actual_token0_gas_payment() {
    let a = snap("angstrom_order_access");
    let d = snap("angstrom_ethereum_directory");
    let cash = a["tob"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["internal"] == false)
        .unwrap();
    let r = d["samples"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["tx"]["hash"] == cash["tx"])
        .unwrap();
    assert_ne!(r["tx"]["from"], cash["payer"]);
    let hook = &r["tx"]["to"];
    let logs = r["receipt"]["logs"].as_array().unwrap();
    assert_eq!(
        transfer(logs, &cash["input"], &cash["payer"], hook),
        936370320150
    );
    assert_eq!(
        transfer(logs, &cash["output"], hook, &cash["recipient"]),
        1581
    );
    assert_eq!(
        cash["input_nominal"].as_u64().unwrap() + cash["gas_token0"].as_u64().unwrap(),
        936370320150
    );
    assert!(
        a["bundle_sample"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["lengths"][4] == 0 && t["lengths"][3].as_u64().unwrap() > 0)
    );
}
