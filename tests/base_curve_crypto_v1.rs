//! Сверка каталога и математики CryptoSwap Base с теми же вызовами, которые делает загрузчик пула.
mod curve_crypto_common;

use alloy_primitives::{Address, Bytes, U256};
use arb::venues::bsc_dex::pools::{load_crypto_v1, verify_crypto_v1::proxy_target};
use curve_crypto_common::{answers, ladder};
use serde_json::Value;

fn archived() -> Value {
    let bytes = include_bytes!("data/first_run_dex_2026-11-26/curve_base_crypto_v1.json.gz");
    serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap()
}

#[test]
fn base_factory_clone_quotes_and_permitted_templates() {
    let archive = archived();
    let profiles: Value = serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let settings = &profiles.as_array().unwrap().iter().find(|p| p["id"] == "curve-base-crypto-v1").unwrap()["settings"];
    let accepted: Vec<Address> = serde_json::from_value(settings["dex.curve_crypto_v1_templates"].clone()).unwrap();
    assert_eq!(settings["dex.factory"], archive["factories"]["crypto"]);
    let mut prices = 0;
    let mut allowed = 0;
    for p in archive["pools"].as_array().unwrap() {
        let market: Address = serde_json::from_value(p["pool"].clone()).unwrap();
        let code: Bytes = serde_json::from_value(p["code"].clone()).unwrap();
        let target = proxy_target(&code).unwrap();
        assert_eq!(serde_json::json!(target),p["template"]);
        assert!(archive["registry"]["crypto"].as_array().unwrap().contains(&p["pool"]));
        if !accepted.contains(&target) { continue; }
        allowed += 1;
        if p["get_dy"].as_array().unwrap().is_empty() { continue; }
        let raw = load_crypto_v1::Raw {
            pool: market, n: 2, got: answers(p,&load_crypto_v1::calls_with(market,2,false)),
            now: archive["timestamp"].as_u64().unwrap(),
            decimals: serde_json::from_value(p["decimals"].clone()).unwrap(), killed: false,
        };
        let (coins,state) = load_crypto_v1::decode(&raw).unwrap();
        assert_eq!(serde_json::json!(coins),p["coins"]);
        prices += ladder("curve-base-crypto-v1",market,p,&state);
    }
    assert_eq!((allowed,prices),(33,88));
}

#[test]
fn four_pools_pay_both_wallet_forms_at_two_sizes() {
    let archive=archived();
    let mut legs = [0;2];
    for c in archive["wallets"].as_array().unwrap() {
        let v5 = c["wire"]=="v5";
        for name in ["forward","reverse"] {
            let r=&c[name];
            if r["stage"]!="paid" { continue; }
            let amount = |v:&Value| v.as_str().unwrap().parse::<U256>().unwrap();
            let paid=amount(&r["debit"]);
            let credit=amount(&r["credit"]);
            assert_eq!(amount(&r["before"][0])-amount(&r["after"][0]),paid);
            assert_eq!(amount(&r["after"][1])-amount(&r["before"][1]),credit);
            assert_eq!(r["strict_reverted"],true);
            if name=="reverse" { assert_eq!(paid,amount(&c["forward"]["credit"])); }
            legs[usize::from(v5)]+=1;
        }
    }
    assert_eq!(legs,[16,16]);
}
