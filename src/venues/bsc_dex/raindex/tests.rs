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
    let (encoded, topic) = if p.version == 3 {
        let c = wire::takeOrdersCall::abi_decode_validate(&action)
            .unwrap()
            .config;
        assert_eq!(c.orders.len(), 1);
        (
            c.orders[0].order.abi_encode(),
            wire::TakeOrder::SIGNATURE_HASH,
        )
    } else if p.version == 4 {
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
        via: None,
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
    let old = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_v3_ethereum.json.gz"
    );
    for (archive, chain, expected) in [
        (base.as_slice(), 8453, (42, 7)),
        (eth.as_slice(), 1, (3, 0)),
        (old.as_slice(), 1, (4, 0)),
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

#[test]
fn stablecoin_route_restores_receipt_for_book_fill_and_conversion_of_its_change() {
    let a = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_cash_ethereum.json.gz"
    );
    let data: Value = serde_json::from_reader(GzDecoder::new(a.as_slice())).unwrap();
    let role = cash::Role {
        wrapper: from_value(data["wrapper"].clone()).unwrap(),
        hash: from_value(data["wrapperCode"].clone()).unwrap(),
        router: from_value(data["router"].clone()).unwrap(),
        pair: from_value(data["pair"].clone()).unwrap(),
        cash: from_value(data["cash"].clone()).unwrap(),
        bridge: from_value(data["bridge"].clone()).unwrap(),
    };
    let bare = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_ethereum_protected_wallets.json.gz"
    );
    let book_info: Value = serde_json::from_reader(GzDecoder::new(bare.as_slice())).unwrap();
    let book = book_info["modules"]
        .as_object()
        .unwrap()
        .values()
        .find(|x| x["version"] == 4)
        .unwrap();
    let p = Profile {
        book: from_value(book["book"].clone()).unwrap(),
        book_hash: B256::ZERO,
        chain: 1,
        version: 4,
        module: from_value(data["wallet"].clone()).unwrap(),
        module_hash: from_value(data["walletCode"].clone()).unwrap(),
    };
    let owner = from_value(data["payer"].clone()).unwrap();
    let mut with_return = 0;
    let rows = data["cases"].as_array().unwrap();
    for row in rows {
        let action: Bytes = from_value(row["action"].clone()).unwrap();
        let config = wire::takeOrders2Call::abi_decode_validate(&action)
            .unwrap()
            .config;
        let hash = from_value(row["order"].clone()).unwrap();
        let raw = config.orders[0].order.abi_encode();
        let leg = Leg {
            via: Some(cash::State::archived(role, 18)),
            p,
            hash,
            order: Arc::new(wire::Order::decode(4, &raw, hash).unwrap()),
            i: config.orders[0].inputIOIndex.to(),
            j: config.orders[0].outputIOIndex.to(),
            input: role.bridge,
            output: from_value(row["asset"].clone()).unwrap(),
        };
        let cap = amount(row, "cap");
        let middle = amount(row, "acquired");
        let net = amount(row, "received");
        let spent = amount(row, "spent");
        let payment =
            payment::Payment::route(&leg, owner, action, 0, (U256::ONE, cap, net), middle);
        let logs: Vec<Log> = from_value(row["logs"].clone()).unwrap();
        assert_eq!(payment.saved.amounts(&logs), Some((spent, net)));
        let separate: Vec<_> = logs
            .into_iter()
            .filter(|l| l.address() != p.module)
            .collect();
        assert!(payment.saved.amounts(&separate).is_none());
        if middle > amount(row, "bookPaid") {
            with_return += 1;
        }
    }
    assert_eq!((rows.len(), with_return), (6, 3));
}

#[test]
fn public_v7_claims_decode_with_their_own_abi_and_do_not_create_spot_prices() {
    let src = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_ethereum_quotes.json.gz"
    );
    let terms = include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_v7_ethereum_terms.json.gz"
    );
    let j: Value = serde_json::from_reader(GzDecoder::new(src.as_slice())).unwrap();
    let t: Value = serde_json::from_reader(GzDecoder::new(terms.as_slice())).unwrap();
    let book = j["books"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["version"] == 7)
        .unwrap();
    let orders = book["live"].as_array().unwrap();
    assert_eq!(orders.len(), 5);
    for row in orders {
        let terms = t["orders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["order"] == row["hash"])
            .unwrap();
        let raw: Bytes = from_value(terms["metadata"].clone()).unwrap();
        assert_eq!(
            alloy_primitives::keccak256(&raw),
            from_value::<B256>(terms["content_hash"].clone()).unwrap()
        );
        let source = String::from_utf8_lossy(&raw);
        assert!(
            source.contains("order-counterparty()")
                && source.contains("signed-context<0 1>()")
                && source.contains("_: 0;")
        );
        let raw: Bytes = from_value(row["order"].clone()).unwrap();
        let hash = from_value(row["hash"].clone()).unwrap();
        let order = wire::Order::decode(7, &raw, hash).unwrap();
        let q = order.quote(0, 0).unwrap();
        assert_eq!(&q[..4], &wire::quote3Call::SELECTOR);
        let action = order.take(0, 0, U256::ONE, 18, true, B256::ZERO).unwrap();
        assert_eq!(&action[..4], &[0xb0, 0x46, 0x0f, 0x07]);
        let c = wire::takeOrders5Call::abi_decode_validate(&action)
            .unwrap()
            .config;
        assert!(c.data.is_empty() && c.orders[0].signedContext.is_empty());
        let response = (true, Quantity::units(U256::ONE, 0).unwrap(), B256::ZERO).abi_encode();
        assert!(order.price(&response).is_none());
    }
}
