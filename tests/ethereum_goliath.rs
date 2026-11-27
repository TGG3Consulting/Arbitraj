//! Goliath: отдельные цена на родителе, доступность дочернего блока и реальная площадка выплаты.
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn registry_quotes_and_wallet_returns_distinguish_the_public_fallback() {
    let capture: Value = serde_json::from_reader(GzDecoder::new(
        include_bytes!("data/first_run_dex_2026-11-26/goliath_ethereum_wallets.json.gz").as_slice(),
    ))
    .unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/goliath_ethereum.json"
    ))
    .unwrap();
    assert_eq!(fixture, capture["registry"]);
    let fallback = &fixture["public_router"]["fallback"];
    let venue = &fixture["venue"];
    assert_ne!(venue, fallback);
    let mut observed = Vec::new();
    for state in capture["states"].as_array().unwrap() {
        let rows = state["rows"].as_array().unwrap();
        let quotes: Vec<_> = rows.iter().filter(|r| r["method"] == "quote").collect();
        assert_eq!(quotes.len(), 8);
        assert!(quotes.iter().all(|r| r["child"]["status"] == "0x0"
            && r["child"]["error"]["message"] == "execution reverted: inactive pair"));
        let open = quotes
            .iter()
            .filter(|r| {
                r["head"]
                    .as_str()
                    .is_some_and(|v| u128::from_str_radix(&v[2..], 16).is_ok_and(|n| n > 0))
            })
            .count();
        let wallet = state["wallet"].as_array().unwrap();
        assert_eq!(wallet.len(), 8);
        for row in wallet {
            assert_eq!(&row["requested"], venue);
            assert_eq!(&row["actual"], fallback);
            assert_eq!(row["fallback"], true);
            let before = row["before"].as_array().unwrap();
            let after = row["after"].as_array().unwrap();
            let debit = before[0].as_u64().unwrap() - after[0].as_u64().unwrap();
            let credit = after[1].as_u64().unwrap() - before[1].as_u64().unwrap();
            assert_eq!(row["amount"], debit);
            assert_eq!(row["received"], credit);
            assert!(credit > 0);
            assert_eq!(before[2..], after[2..]);
            assert_eq!(row["minimum_plus_one"]["status"], "0x0");
            assert_eq!(row["expired"]["code"], 3);
            assert!(
                row["logs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|l| &l["address"] != venue)
            );
        }
        observed.push((state["parent"]["height"].as_u64().unwrap(), open));
    }
    observed.sort_unstable();
    assert_eq!(observed, [(26494977, 8), (26494988, 0)]);
}
