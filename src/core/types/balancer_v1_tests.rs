use super::*;
use serde_json::Value;
use std::{io::Read, str::FromStr};

fn integer(value: &Value) -> U256 {
    U256::from_str(value.as_str().unwrap()).unwrap()
}
fn addr(value: &Value) -> Address {
    Address::from_str(value.as_str().unwrap()).unwrap()
}
fn original() -> Value {
    let archive =
        include_bytes!("../../../tests/data/first_run_dex_2026-11-26/balancer_v1_ethereum.json.gz");
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(archive.as_slice())
        .read_to_end(&mut raw)
        .unwrap();
    serde_json::from_slice(&raw).unwrap()
}
fn market(data: &Value, row: &Value) -> State {
    let m = &data["markets"][row["pool"].as_str().unwrap()];
    State {
        pool: addr(&row["pool"]),
        fee: integer(&m["swap_fee"]),
        public_swap: m["public"].as_bool().unwrap(),
        assets: m["assets"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(address, a)| Asset {
                token: Address::from_str(address).unwrap(),
                recorded: integer(&a["balance"]),
                weight: integer(&a["weight"]),
                // Physical output sufficiency is established by the replay for each paid amount.
                // Its pool snapshot contains accounting balances, not absolute token.balanceOf.
                physical: U256::MAX,
            })
            .collect(),
    }
}
#[test]
fn source_target_calculators_and_swap_guards_match_original_wallets() {
    let data = original();
    let mut counts = [0usize; 2];
    for row in data["payments"].as_array().unwrap() {
        let s = market(&data, row);
        let (a, b) = (addr(&row["source"]), addr(&row["target"]));
        let qty = integer(&row["amount"]);
        let exact = row["exact_input"].as_bool().unwrap();
        assert_eq!(
            s.amount(a, b, qty, exact),
            Some(integer(&row["price"])),
            "{row}"
        );
        let attempt = s.swap(a, b, qty, exact, U256::MAX);
        if let Some(reason) = row["pool_revert"].as_str() {
            assert_eq!(reason, "ERR_MATH_APPROX");
            assert!(attempt.is_none(), "{row}");
            counts[0] += 1;
        } else {
            let fill = attempt.unwrap();
            assert_eq!(
                (fill.spent, fill.received),
                (integer(&row["spent"]), integer(&row["received"]))
            );
            assert_eq!(s.swap(a, b, qty, exact, fill.spot_after), Some(fill));
            assert_eq!(s.swap(a, b, qty, exact, fill.spot_after - U256::ONE), None);
            counts[1] += 1;
        }
    }
    assert_eq!(counts, [7, 21]);
}
#[test]
fn donations_do_not_change_price_and_each_projection_consumes_the_same_pool_inventory() {
    let data = original();
    let row = &data["payments"][0];
    let mut s = market(&data, row);
    let (a, b) = (addr(&row["source"]), addr(&row["target"]));
    let qty = integer(&row["amount"]);
    let exact = row["exact_input"].as_bool().unwrap();
    let fill = s.swap(a, b, qty, exact, U256::MAX).unwrap();
    let (i, j) = s.indices(a, b).unwrap();
    s.assets[i].physical = s.assets[i].recorded + U256::ONE;
    s.assets[j].physical = fill.received;
    assert_eq!(s.swap(a, b, qty, exact, U256::MAX), Some(fill));
    let next = s.apply(a, b, fill).unwrap();
    assert_eq!(next.assets[j].physical, U256::ZERO);
    assert_eq!(next.assets[i].physical, next.assets[i].recorded + U256::ONE);
    assert!(next.swap(a, b, qty, exact, U256::MAX).is_none());
    s.assets[j].physical -= U256::ONE;
    assert!(s.swap(a, b, qty, exact, U256::MAX).is_none());
    s.assets[j].physical = U256::MAX;
    s.public_swap = false;
    assert!(s.swap(a, b, qty, exact, U256::MAX).is_none());
}
