//! Привязка исходных ответов BSC к кошельку и блоку в корпусе О-65.
//! Проверяет сами исходные данные. Допуск продажи проверяют тесты адаптера.
use alloy_primitives::{Address, B256};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::fs::File;

/// У запроса совпадают номер или канонический хеш блока, у ответа — JSON-RPC ID.
fn result_at<'a>(record: &'a Value, method: &str, capture: &Value) -> &'a Value {
    let request = &record["request"];
    assert_eq!(request["method"], method);
    let tag = request["params"].as_array().unwrap().last().unwrap();
    if tag.is_object() {
        assert_eq!(tag["blockHash"], capture["blockHash"]);
        assert_eq!(tag["requireCanonical"], true);
    } else {
        assert_eq!(tag, &capture["block"]);
    }
    let response = &record["response"];
    assert_eq!(request["id"], response["id"]);
    assert!(response.get("error").is_none());
    response.get("result").unwrap()
}

#[test]
fn recorded_reads_are_bound_to_the_same_block_token_and_two_holders() {
    for name in ["era", "usdt", "wbnb", "nxt", "pig", "smars", "xvs"] {
        let file = File::open(format!(
            "tests/data/presell_bsc_2026-11-27/{name}_with_code.json.gz"
        ))
        .unwrap();
        let capture: Value = serde_json::from_reader(GzDecoder::new(file)).unwrap();
        assert_eq!(capture["chain_id"], 56);
        assert!(
            capture["blockHash"]
                .as_str()
                .unwrap()
                .parse::<B256>()
                .is_ok()
        );
        let code = &capture["code"];
        assert_eq!(code["request"]["params"][0], capture["token"]);
        assert!(
            result_at(code, "eth_getCode", &capture)
                .as_str()
                .unwrap()
                .len()
                > 2
        );
        let samples = capture["samples"].as_array().unwrap();
        assert_eq!(samples.len(), 2);
        assert_ne!(samples[0]["holder"], samples[1]["holder"]);
        for sample in samples {
            let access = &sample["access"];
            let balance = &sample["balance"];
            let tx = &access["request"]["params"][0];
            assert_eq!(*tx, balance["request"]["params"][0]);
            assert_eq!(tx["from"], samples[0]["holder"]);
            assert_eq!(tx["to"], capture["token"]);
            let holder: Address = sample["holder"].as_str().unwrap().parse().unwrap();
            assert_eq!(tx["data"], format!("0x70a08231{:x}", holder.into_word()));
            let list = result_at(access, "eth_createAccessList", &capture);
            assert!(list.get("error").is_none());
            let have = result_at(balance, "eth_call", &capture);
            assert!(have.as_str().unwrap().parse::<B256>().is_ok());
            if let Some(storage) = sample.get("storage") {
                assert_eq!(result_at(storage, "eth_getStorageAt", &capture), have);
                let params = &storage["request"]["params"];
                assert!(list["accessList"].as_array().unwrap().iter().any(|entry| {
                    entry["address"] == params[0]
                        && entry["storageKeys"]
                            .as_array()
                            .unwrap()
                            .contains(&params[1])
                }));
            }
        }
    }
}
