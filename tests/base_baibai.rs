//! Плательщик, индивидуальная комиссия, остаток кастодиана и срок кривой BaiBai на архивном хеше.
use alloy_primitives::U256;
use arb::venues::bsc_dex::pools::families;
use serde_json::Value;

fn amount(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn exact_wallet_payout_and_curve_expiry_even_without_a_new_sequence() {
    let signal = families::make("baibai", "baibai").unwrap();
    let trade = families::make("baibai", "baibai_module").unwrap();
    assert!(!signal.tradable && trade.tradable && trade.direct_module());
    for fixture in [
        include_str!("data/first_run_dex_2026-11-26/baibai_base.json"),
        include_str!("data/first_run_dex_2026-11-26/baibai_nvdac_base.json"),
    ] {
        let sample: Value = serde_json::from_str(fixture).unwrap();
        check_market(&sample);
    }
}

fn check_market(sample: &Value) {
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

#[test]
fn compatible_two_market_release_cannot_spend_for_baibai() {
    use alloy_primitives::b256;
    use arb::venues::bsc_dex::pools::direct_module;
    let old = b256!("ce587f0be477a2a6a9bd8b72a5123b9474709cf99c4c958cf777e698d7284403");
    assert!(direct_module::accepts(8453, "elfomofi", old));
    assert!(direct_module::accepts(8453, "lunarbase", old));
    assert!(!direct_module::accepts(8453, "baibai", old));
}

#[test]
fn same_wallet_can_sell_the_delivered_stock_in_the_updated_book() {
    let native: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/baibai_nvdac_roundtrip.json"
    ))
    .unwrap();
    let via: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/base_pmm_executor_baibai_roundtrip.json"
    ))
    .unwrap();
    assert_eq!(native["wallet"], via["wallet"]);
    for sample in [native, via] {
        let r = &sample["results"];
        let n = |k: &str| amount(&r[k]["returnData"]);
        assert_eq!(n("coin_before"), n("coin_after"));
        assert_eq!(
            n("coin_middle") - n("coin_before"),
            amount(&sample["purchased"])
        );
        assert_eq!(n("usd_before") - n("usd_middle"), amount(&sample["spent"]));
        assert_eq!(
            n("usd_after") - n("usd_middle"),
            amount(&sample["returned"])
        );
        assert_eq!(r["above_sell"]["status"], "0x0");
    }
}
