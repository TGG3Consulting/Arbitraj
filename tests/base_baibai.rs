//! Плательщик, индивидуальная комиссия, остаток кастодиана и срок кривой BaiBai на архивном хеше.
use alloy_primitives::U256;
use arb::venues::bsc_dex::pools::families;
use serde_json::Value;

fn amount(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn exact_wallet_payout_and_curve_expiry_even_without_a_new_sequence() {
    let sample: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/baibai_base.json"
    ))
    .unwrap();
    let family = families::make("baibai", "baibai").unwrap();
    assert!(!family.tradable && !family.direct_module());
    let cases = sample["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for c in cases {
        let live = &c["actual"]["results"];
        let stale = &c["expired"]["results"];
        let value = |key: &str| amount(&live[key]["returnData"]);
        let dx = amount(&c["amount"]);
        let net = amount(&c["out"]);
        let fee = amount(&c["fee"]);
        assert!(fee > U256::ZERO);
        assert_eq!(value("gross"), net + fee);
        assert_eq!(value("exact"), net);
        assert_eq!(value("wallet_in_before") - value("wallet_in_after"), dx);
        assert_eq!(value("vault_in_after") - value("vault_in_before"), dx);
        assert_eq!(value("wallet_out_after") - value("wallet_out_before"), net);
        assert_eq!(value("vault_out_before") - value("vault_out_after"), net);
        assert_eq!(live["above"]["status"], "0x0");
        assert_eq!(stale["state"]["returnData"], live["state"]["returnData"]);
        let quote = stale["net"]["returnData"]
            .as_str()
            .unwrap()
            .strip_prefix("0x")
            .unwrap();
        assert_eq!(quote.len(), 128);
        assert!(quote.bytes().all(|b| b == b'0'));
        assert_eq!(stale["expired"]["status"], "0x0");
    }
}
