//! Публичные поля компилятора и копия ответа BSC. Второй список ниже —
//! синтетический контроль фильтра, а не записанный вызов того же блока.
use super::*;
use serde_json::{Value, json};
const FILE: &str =
    include_str!("../../../../tests/data/presell_bsc_2026-11-27/verified_layouts.json");
const ACCESS: &str =
    include_str!("../../../../tests/data/presell_bsc_2026-11-27/access_lists.json");

#[test]
fn layouts_must_be_complete_uint256_fields_in_the_matching_compiler_output() {
    let raw: Vec<Value> = serde_json::from_str(FILE).unwrap();
    for source in raw {
        let hash: B256 = source["runtimeKeccak"].as_str().unwrap().parse().unwrap();
        let deployed: Address = source["address"].as_str().unwrap().parse().unwrap();
        let matching: Vec<_> = DEPLOYMENTS
            .iter()
            .filter(|d| d.chain_id == 56 && d.address == deployed)
            .collect();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].runtime_hash, hash);
        if source["storage"].as_array().unwrap().is_empty() {
            assert_eq!(declaration(hash), Some(Declaration::TransparentErc1967));
            continue;
        }
        let field = &source["storage"][0];
        assert_eq!(field["label"], "_balances");
        assert_eq!(field["offset"], 0);
        assert_eq!(source["type"]["key"], "t_address");
        assert_eq!(source["type"]["value"], "t_uint256");
        assert_eq!(source["type"]["encoding"], "mapping");
        assert_eq!(
            full_word(hash).unwrap().index,
            field["slot"].as_str().unwrap().parse::<u64>().unwrap()
        );
    }
    assert_eq!(full_word(B256::ZERO), None);
}

#[test]
fn proxy_management_keys_are_never_balance_candidates() {
    let sample: Value = serde_json::from_str(ACCESS).unwrap();
    let request = &sample["eraRequest"]["params"][0];
    let wallet: Address = request["from"].as_str().unwrap().parse().unwrap();
    let token: Address = request["to"].as_str().unwrap().parse().unwrap();
    let a: Access = serde_json::from_value(sample["era"]["result"].clone()).unwrap();
    let imp = a
        .access_list
        .iter()
        .find(|i| i.address != token)
        .unwrap()
        .address;
    let owner = &a
        .access_list
        .iter()
        .find(|i| i.address == token)
        .unwrap()
        .storage_keys;
    let slot = Slot {
        index: 262,
        layout: Layout::Solidity,
    };
    let keys = [
        presell_slot::key(slot, wallet),
        presell_slot::key(slot, Address::ZERO),
    ];
    let fixed: Vec<_> = owner.iter().copied().filter(|k| *k != keys[0]).collect();
    let mut second = sample["era"]["result"].clone();
    let fields = second["accessList"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["address"] == request["to"])
        .unwrap();
    let mut changed = fixed.clone();
    changed.push(keys[1]);
    fields["storageKeys"] = json!(changed);
    let b: Access = serde_json::from_value(second.clone()).unwrap();
    assert!(matches(&a, &b, token, keys, Some(imp)));
    for other in fixed {
        assert!(!matches(&a, &b, token, [other, keys[1]], Some(imp)));
    }
    assert!(!matches(&a, &b, token, keys, Some(Address::ZERO)));
    second["accessList"][0]["storageKeys"]
        .as_array_mut()
        .unwrap()
        .push(json!(B256::ZERO));
    assert!(!matches(
        &a,
        &serde_json::from_value(second).unwrap(),
        token,
        keys,
        Some(imp)
    ));
}

#[test]
fn reflection_fields_in_recorded_code_do_not_admit_balance_credit() {
    let evidence: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../tests/data/presell_bsc_2026-11-27/reflection_layouts.json"
    ))
    .unwrap();
    assert_eq!(evidence.len(), 2);
    for token in evidence {
        for name in ["_rOwned", "_tOwned", "_isExcluded"] {
            assert!(
                token["fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|f| f["name"] == name)
            );
        }
        let hash: B256 = token["runtime"].as_str().unwrap().parse().unwrap();
        assert!(declaration(hash).is_none());
    }
}

#[test]
fn packed_or_malformed_word_never_funds_a_sale() {
    let bal = B256::from(U256::from(12).to_be_bytes::<32>());
    let mut packed = bal;
    packed.0[0] = 1;
    assert!(balance_word(bal.as_slice(), packed).is_err());
    assert!(balance_word(&bal.as_slice()[1..], bal).is_err());
    assert_eq!(balance_word(bal.as_slice(), bal), Ok(U256::from(12)));
}
