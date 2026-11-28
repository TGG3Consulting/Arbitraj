//! Сопоставляет обмен в канонической книге Base V6 с оплатой того же ордера через RaindexWallet.
//! Матрица GitHub читает две независимые архивные симуляции на общем родительском блоке.
use std::collections::BTreeMap;
use serde_json::Value;

/// Привязывает каждую пробу к конкретному ордеру и паре его индексов, включая проверку вместимости.
fn cases(archive: &Value) -> BTreeMap<String, &Value> {
    let mut rows = BTreeMap::new();
    for market in archive["pairs"].as_array().unwrap() {
        let key_for = |mode: &str, scale: &Value, row: &Value| {
            format!("{}:{}:{}:{}:{}:{}:{}:{}", row["book"], row["order"], row["i"], row["j"],
                market["pay"], market["receive"], mode, scale)
        };
        for c in market["cases"].as_array().unwrap() {
            for mode in ["by_debit", "by_output"] {
                let row = &c[mode];
                assert!(rows.insert(key_for(mode, &c["scale"], row), row).is_none());
            }
        }
        if let Some(r) = market.get("partial_capacity") {
            assert!(rows.insert(key_for("partial_capacity", &Value::Null, r), r).is_none());
        }
    }
    rows
}

#[test]
fn protected_wallet_and_direct_book_agree_for_the_same_orders_sizes_and_limits() {
    let read = |bytes: &[u8]| -> Value {
        serde_json::from_reader(flate2::read::GzDecoder::new(bytes)).unwrap()
    };
    let direct = read(include_bytes!("data/first_run_dex_2026-11-26/raindex_base_wallets.json.gz"));
    let protected = read(include_bytes!("data/first_run_dex_2026-11-26/raindex_base_protected_wallets.json.gz"));
    assert_eq!(direct["selector"], protected["selector"]);
    assert_eq!(direct["payer"], protected["payer"]);
    let expected = cases(&direct);
    let actual = cases(&protected);
    assert!(expected.keys().eq(actual.keys()));
    let (mut full, mut partial, mut denied) = (0, 0, 0);
    for (key, row) in actual {
        let previous = expected[&key];
        for field in ["size", "cap", "partial", "fixed_receive", "stage"] {
            assert_eq!(row[field], previous[field], "{key} / {field}");
        }
        match row["stage"].as_str().unwrap() {
            "paid" => {
                assert_eq!(row["paid"], previous["paid"]);
                assert_eq!(row["net"], previous["net"]);
                if row["partial"] == true { partial += 1 } else { full += 1 }
            }
            "revert" => {
                assert!(row["error"]["code"].is_number());
                assert!(previous["error"]["code"].is_number());
                denied += 1;
            }
            stage => panic!("unknown stage {stage}"),
        }
    }
    assert_eq!((full, partial, denied), (42, 7, 12));
}
