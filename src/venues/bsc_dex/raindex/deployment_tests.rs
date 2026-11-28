//! Реестр Rain Base, остатки трёх расчётных ERC20 и фактически восстановленные заявки.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn registry() -> Value {
    serde_json::from_reader(GzDecoder::new(&include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_deployment_balances.json.gz"
    )[..])).unwrap()
}

#[test]
fn canonical_rain_deployments_and_unprofiled_cash_books() {
    let doc = registry();
    let cash: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/base_usdt_cycle.json.gz")
            [..],
    ))
    .unwrap();
    let seed: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_v5_seed.json.gz"
        )[..],
    ))
    .unwrap();
    let quotes: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/raindex_base_quotes.json.gz"
        )[..],
    ))
    .unwrap();
    assert_eq!(doc["chain_id"], 8453);
    assert_eq!(doc["height"], cash["block"]);
    assert_eq!(doc["selector"], cash["selector"]);
    assert_eq!(doc["selector"], quotes["anchor"]);
    assert_eq!(doc["tokens"]["USDC"], cash["working"]);
    assert_eq!(doc["tokens"]["USDT"], cash["cash"]);
    assert_eq!(doc["tokens"]["WETH"], cash["assets"][2]);
    let mut books = BTreeMap::new();
    let mut generations = BTreeMap::new();
    let mut funded = BTreeSet::new();
    let mut positive = [0; 3];
    for row in doc["books"].as_array().unwrap() {
        let address: Address = from_value(row["book"].clone()).unwrap();
        let version = row["version"].as_u64().unwrap();
        let selector = match version {
            3 => wire::takeOrdersCall::SELECTOR,
            4 => wire::takeOrders2Call::SELECTOR,
            5 => wire::takeOrders3Call::SELECTOR,
            6 => wire::takeOrders4Call::SELECTOR,
            7 => wire::takeOrders5Call::SELECTOR,
            _ => panic!("unrecognized interface version"),
        };
        assert_eq!(row["take_selector"], hex::encode(selector));
        assert_eq!(row["push4_take"], true);
        assert!(row["code_bytes"].as_u64().unwrap() > 0);
        assert!(books.insert(address, &row["code_hash"]).is_none());
        *generations.entry(version).or_insert(0) += 1;
        for (i, token) in ["USDC", "USDT", "WETH"].into_iter().enumerate() {
            if !amount(&row["balances"], token).is_zero() {
                positive[i] += 1;
                funded.insert(address);
            }
        }
    }
    assert_eq!(
        generations,
        BTreeMap::from([(3, 3), (4, 11), (5, 5), (6, 18), (7, 1)])
    );
    assert_eq!(positive, [13, 0, 5]);
    for (book, hash) in [
        (&seed["book"], &seed["book_hash"]),
        (&quotes["book"], &quotes["runtime"]),
    ] {
        let address: Address = from_value(book.clone()).unwrap();
        assert_eq!(books[&address], hash);
        assert!(funded.remove(&address));
    }
    let mut orders = 0;
    let mut live = 0;
    for sample in doc["order_samples"].as_array().unwrap() {
        let address: Address = from_value(sample["book"].clone()).unwrap();
        assert!(funded.remove(&address));
        assert_eq!(books[&address], &sample["runtime"]);
        let version = sample["version"].as_u64().unwrap() as u8;
        for r in sample["orders"].as_array().unwrap() {
            let hash = from_value(r["hash"].clone()).unwrap();
            let bytes: Bytes = from_value(r["order"].clone()).unwrap();
            let order = wire::Order::decode(version, &bytes, hash).unwrap();
            assert_eq!(
                order.owner(),
                from_value::<Address>(r["owner"].clone()).unwrap()
            );
            assert!(
                sample["receipts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["order"] == r["hash"])
            );
            let (inputs, outputs) = order.pairs();
            for leg in r["quotes"].as_array().unwrap() {
                assert_eq!(
                    inputs[leg["i"].as_u64().unwrap() as usize],
                    from_value::<Address>(leg["pay"].clone()).unwrap()
                );
                assert_eq!(
                    outputs[leg["j"].as_u64().unwrap() as usize],
                    from_value::<Address>(leg["receive"].clone()).unwrap()
                );
            }
            orders += 1;
            if r["exists"] == true {
                live += 1;
            }
        }
    }
    assert!(funded.is_empty());
    assert_eq!((orders, live), (18, 16));
}

#[test]
fn funded_base_books_bind_receipt_amounts_and_rollback() {
    let doc = registry();
    let replay = &doc["sample_replay"];
    let bare = &replay["direct"];
    let protected = &replay["protected"];
    assert_eq!(bare["selector"], doc["selector"]);
    assert_eq!(protected["selector"], bare["selector"]);
    assert_eq!(protected["wlth_funding"], bare["wlth_funding"]);
    let funding = &bare["wlth_funding"];
    let at = |row: &Value, key: &str, index: usize| -> U256 {
        row[key][index].as_str().unwrap().parse().unwrap()
    };
    assert_eq!(
        at(funding, "before", 0) - at(funding, "after", 0),
        U256::from(1_000_000)
    );
    assert!(at(funding, "after", 1) > at(funding, "before", 1));
    let owner: Address = from_value(protected["payer"].clone()).unwrap();
    let mut paid = BTreeMap::new();
    let mut rejected = 0;
    let bare_cases = bare["cases"].as_array().unwrap();
    let cases = protected["cases"].as_array().unwrap();
    assert_eq!(bare_cases.len(), cases.len());
    for (direct, case) in bare_cases.iter().zip(cases) {
        for key in [
            "book", "version", "order", "i", "j", "pay", "receive", "size", "stage",
        ] {
            assert_eq!(direct[key], case[key]);
        }
        if case["stage"] == "revert" {
            for row in [direct, case] {
                assert_eq!(row["error"]["code"], 3);
                assert!(row["before"].as_array().unwrap().len() >= 2);
                assert_eq!(row["before"], row["after"]);
            }
            rejected += 1;
            continue;
        }
        check(case, owner, 8453);
        let accepted = &case["accepted_payment"];
        assert_eq!(accepted["from"], protected["payer"]);
        assert_eq!(accepted["to"], case["target"]);
        let data: Bytes = from_value(accepted["data"].clone()).unwrap();
        let call = wire::fillCall::abi_decode_validate(&data).unwrap();
        assert_eq!(
            call.action,
            from_value::<Bytes>(case["action"].clone()).unwrap()
        );
        assert_eq!(
            call.l.input,
            from_value::<Address>(case["pay"].clone()).unwrap()
        );
        assert_eq!(
            call.l.output,
            from_value::<Address>(case["receive"].clone()).unwrap()
        );
        assert_eq!(call.l.minimumDebit, amount(case, "paid"));
        assert_eq!(call.l.maximumDebit, amount(case, "paid"));
        assert_eq!(call.l.minimumCredit, amount(case, "net"));
        assert_eq!(
            call.l.deadline,
            U256::from(case["accepted_deadline"].as_u64().unwrap())
        );
        assert_eq!(case["paid"], direct["paid"]);
        assert_eq!(case["net"], direct["net"]);
        let bounds = case["failed_bounds"].as_array().unwrap();
        assert_eq!(bounds.len(), 3);
        assert_eq!(amount(&bounds[0], "floor"), amount(case, "net") + U256::ONE);
        assert_eq!(amount(&bounds[1], "cap") + U256::ONE, amount(case, "paid"));
        assert!(bounds[2]["deadline"].as_u64().unwrap() < bounds[0]["deadline"].as_u64().unwrap());
        for failed in bounds {
            assert_eq!(failed["error"]["code"], 3);
            assert_eq!(failed["before"], case["before"]);
            assert_eq!(failed["before"], failed["after"]);
        }
        let version = case["version"].as_u64().unwrap();
        let net = amount(case, "net").to::<u64>();
        let spent = amount(case, "paid").to::<u64>();
        assert!(paid.insert((version, net), spent).is_none());
    }
    assert_eq!(rejected, 14);
    assert_eq!(
        paid,
        BTreeMap::from([
            ((3, 1000), 516_440_961_769_448_410),
            ((3, 10000), 5_164_409_617_694_484_095),
            ((6, 1000), 299_000_000_000),
            ((6, 10000), 2_990_000_000_000),
        ])
    );
}
