//! Доставка через внешнего отправителя. 19 подлинных поручений, 18 групп балансов на границах блоков.
use super::super::{
    FillOrder, PMM,
    tests::{archive, logs},
    unique,
};
use alloy_primitives::{B256, U256};
use std::collections::{BTreeMap, HashMap};

#[test]
fn tokenlon_relayed_owner_balances_and_overlapping_payments() {
    let historic = archive(include_bytes!(
        "../../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_expired_pmm.json.gz"
    ));
    let state = archive(include_bytes!(
        "../../../../../tests/data/first_run_dex_2026-11-26/tokenlon_ethereum_relayed_balances.json.gz"
    ));
    let originals: HashMap<_, _> = historic["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["tx"]["hash"].as_str().unwrap(), r))
        .collect();
    let mut groups = BTreeMap::new();
    let mut native = 0;
    for row in state["rows"].as_array().unwrap() {
        let tx = row["hash"].as_str().unwrap();
        let r = originals[tx];
        assert_eq!(r["tx"]["hash"], r["canonical"]);
        assert_eq!(row["block"], r["tx"]["blockHash"]);
        assert_eq!(row["delivery"], r["tx"]["from"]);
        let pay = unique::<FillOrder>(&logs(&r["logs"]), PMM).unwrap();
        assert_ne!(row["delivery"], row["owner"]);
        assert_eq!(row["owner"], serde_json::json!(pay.userAddr));
        assert_eq!(pay.receiverAddr, pay.userAddr);
        assert_eq!(row["input"], serde_json::json!(pay.takerAssetAddr));
        assert_eq!(row["output"], serde_json::json!(pay.makerAssetAddr));
        native += usize::from(pay.makerAssetAddr == super::WETH);
        let word =
            |key: &str, col: usize| serde_json::from_value::<U256>(row[key][col].clone()).unwrap();
        assert_eq!(row["before"][2], "0x");
        assert_eq!(row["after"][2], "0x");
        assert_eq!(word("before", 3), word("after", 3));
        let balances = (
            word("before", 0) - word("after", 0),
            word("after", 1) - word("before", 1),
        );
        let block: B256 = serde_json::from_value(row["block"].clone()).unwrap();
        let key = (block, pay.userAddr, pay.takerAssetAddr, pay.makerAssetAddr);
        let group = groups
            .entry(key)
            .or_insert((balances, (U256::ZERO, U256::ZERO), 0));
        assert_eq!(group.0, balances);
        group.1.0 += pay.takerAssetAmount;
        group.1.1 += pay.settleAmount;
        group.2 += 1;
    }
    let mut singles = 0;
    let mut doubles = 0;
    for (measured, events, count) in groups.into_values() {
        assert_eq!(measured, events);
        match count {
            1 => singles += 1,
            2 => doubles += 1,
            _ => panic!("Неожиданное число поручений в группе"),
        }
    }
    assert_eq!((singles, doubles, native), (17, 1, 13));
}
