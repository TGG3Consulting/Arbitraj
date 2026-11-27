//! Сверка исходного хеша ордера, двух событий и физических остатков по обеим сетям.
use super::*;
use alloy::{
    rpc::types::Log,
    sol_types::{SolCall, SolEvent, SolValue},
};
use flate2::read::GzDecoder;
use serde_json::{Value, from_value};

fn amount(row: &Value, field: &str) -> U256 {
    row[field].as_str().unwrap().parse().unwrap()
}

fn check(row: &Value, owner: Address, chain: u64) {
    let module = from_value(row["target"].clone()).unwrap();
    let book = from_value(row["book"].clone()).unwrap();
    let p = Profile {
        module,
        book,
        chain,
        version: row["version"].as_u64().unwrap() as u8,
        book_hash: B256::ZERO,
        module_hash: B256::ZERO,
    };
    let action: Bytes = from_value(row["action"].clone()).unwrap();
    let (encoded, topic) = if p.version == 4 {
        let c = wire::takeOrders2Call::abi_decode_validate(&action)
            .unwrap()
            .config;
        assert_eq!(c.orders.len(), 1);
        (
            c.orders[0].order.abi_encode(),
            wire::TakeOrderV2::SIGNATURE_HASH,
        )
    } else {
        let c = wire::takeOrders4Call::abi_decode_validate(&action)
            .unwrap()
            .config;
        assert_eq!(c.orders.len(), 1);
        (
            c.orders[0].order.abi_encode(),
            wire::TakeOrderV3::SIGNATURE_HASH,
        )
    };
    let hash = from_value(row["order"].clone()).unwrap();
    let order = Arc::new(wire::Order::decode(p.version, &encoded, hash).unwrap());
    let leg = Leg {
        p,
        hash,
        order,
        i: row["i"].as_u64().unwrap() as usize,
        j: row["j"].as_u64().unwrap() as usize,
        input: from_value(row["pay"].clone()).unwrap(),
        output: from_value(row["receive"].clone()).unwrap(),
    };
    let spent = amount(row, "paid");
    let net = amount(row, "net");
    let payment = payment::Payment::new(&leg, owner, action, 0, (spent, spent, net));
    let mut logs: Vec<Log> = from_value(row["logs"].clone()).unwrap();
    assert_eq!(payment.saved.amounts(&logs), Some((spent, net)));
    let holdings = |key: &str| {
        row[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().parse::<U256>().unwrap())
            .collect::<Vec<_>>()
    };
    let before = holdings("before");
    let after = holdings("after");
    assert_eq!(before[0] - after[0], spent);
    assert_eq!(after[1] - before[1], net);
    assert_eq!(before[2..], after[2..]);
    let repeated = logs
        .iter()
        .find(|e| e.address() == book && e.topics().first() == Some(&topic))
        .unwrap()
        .clone();
    logs.push(repeated);
    assert!(payment.saved.amounts(&logs).is_none());
}

#[test]
fn original_order_and_one_payment_match_own_wallet_on_both_networks() {
    let base = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_protected_wallets.json.gz"
    );
    let eth = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_ethereum_protected_wallets.json.gz"
    );
    for (archive, chain, expected) in [
        (base.as_slice(), 8453, (42, 7)),
        (eth.as_slice(), 1, (3, 0)),
    ] {
        let j: Value = serde_json::from_reader(GzDecoder::new(archive)).unwrap();
        let owner = from_value(j["payer"].clone()).unwrap();
        let mut counts = (0, 0);
        for pair in j["pairs"].as_array().unwrap() {
            for case in pair["cases"].as_array().unwrap() {
                for row in case
                    .as_object()
                    .unwrap()
                    .values()
                    .filter(|r| r["stage"] == "paid")
                {
                    check(row, owner, chain);
                    counts.0 += 1;
                }
            }
            let partial = &pair["partial_capacity"];
            if partial["stage"] == "paid" {
                assert!(amount(partial, "net") < amount(partial, "size"));
                assert!(amount(partial, "paid") < amount(partial, "cap"));
                check(partial, owner, chain);
                counts.1 += 1;
            }
        }
        assert_eq!(counts, expected);
    }
}
