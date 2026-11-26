//! Каталог обеих кривых, обеспечение и фактические обе ноги V5 на архивном состоянии Base.
use alloy_primitives::{Address, B256, U256, keccak256};
use arb::venues::bsc_dex::{
    math::solidly_v1_math::volatile_amount_out,
    pools::{families, family_variant::Variant},
};
use flate2::read::GzDecoder;
use serde_json::Value;

fn n(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn factory_escrow_and_recipient_fees_match_measured_wallets() {
    let reader = GzDecoder::new(
        &include_bytes!("data/first_run_dex_2026-11-26/leetswap_base_directory.json.gz")[..],
    );
    let book: Value = serde_json::from_reader(reader).unwrap();
    let cases: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/leetswap_base_swaps.json"
    ))
    .unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let cfg = &profiles
        .iter()
        .find(|p| p["id"] == "leetswap-base" && p["chain_id"] == 8453)
        .unwrap()["settings"];
    assert_eq!(book["selector"], cases["selector"]);
    assert_eq!(book["factory"], cfg["dex.factory"]);
    assert_eq!(
        families::make("leet", cfg["dex.kind"].as_str().unwrap())
            .unwrap()
            .variant,
        Variant::LeetSwap
    );
    let factory: Address = book["factory"].as_str().unwrap().parse().unwrap();
    let init: B256 = book["pair_init_hash"].as_str().unwrap().parse().unwrap();
    let markets = book["markets"].as_array().unwrap();
    assert_eq!(markets.len(), 4505);
    assert_eq!(markets.iter().filter(|m| m["stable"] == true).count(), 0);
    let mut funded = 0;
    for m in markets {
        let mut packed = [0; 41];
        for i in 0..2 {
            let token = m["tokens"][i].as_str().unwrap().parse::<Address>().unwrap();
            packed[i * 20..(i + 1) * 20].copy_from_slice(token.as_slice());
        }
        packed[40] = u8::from(m["stable"].as_bool().unwrap());
        assert_eq!(
            factory.create2(keccak256(packed), init),
            m["pool"].as_str().unwrap().parse::<Address>().unwrap()
        );
        funded += usize::from((0..2).all(|i| !n(&m["balances"][i]).is_zero()));
    }
    assert_eq!(funded, 4115);
    let mut executions = 0;
    let mut taxed = 0;
    for c in cases["cases"].as_array().unwrap() {
        let m = &c["market"];
        let Some(legs) = c["result"].as_object() else {
            assert!(c["rejection"].is_string());
            continue;
        };
        assert_eq!(m["reserves"], m["balances"]);
        for side in ["buy", "sell"] {
            let leg = &legs[side];
            let state = &leg["exact"];
            let i = usize::from(leg["input"] != m["tokens"][0]);
            let fee = &state["fees_by_recipient"];
            assert_eq!(fee[0], fee[1]);
            let out = volatile_amount_out(
                n(&leg["amount"]),
                n(&state["reserves"][i]),
                n(&state["reserves"][1 - i]),
                n(&fee[0]),
            )
            .unwrap();
            let received = n(&state["received"]);
            assert!(received <= out);
            taxed += usize::from(received < out);
            assert_eq!(out, n(&state["recipient_quote"]));
            assert_eq!(leg["amount"], state["spent"]);
            assert_eq!(leg["plus_one"]["status"], "0x0");
            assert_eq!(leg["plus_one"]["spent"], "0");
            assert_eq!(leg["plus_one"]["received"], "0");
            executions += 1;
        }
    }
    assert_eq!(executions, 12);
    assert_eq!(taxed, 2);
}
