//! Файл: tests/base_elfomofi.rs. Проверяет ABI реестра, границу vault и полные дельты с канонического Base.
use alloy_primitives::{Address, U256};
use arb::venues::bsc_dex::pools::elfomo;
use serde_json::Value;

#[test]
fn canonical_pairs_pay_exact_amount_or_have_no_executable_quote() {
    let data: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/elfomofi_base.json"
    ))
    .unwrap();
    let number = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let registry = &data["reads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["signature"] == "getSupportedPairs()")
        .unwrap()["response"];
    let raw = hex::decode(registry.as_str().unwrap().strip_prefix("0x").unwrap()).unwrap();
    let mut amounts = (0, 0);
    for c in data["cases"].as_array().unwrap() {
        let (input, output) = (addr(&c["token_in"]), addr(&c["token_out"]));
        let confirmed = elfomo::listed(&raw, input, output).unwrap()
            || elfomo::listed(&raw, output, input).unwrap();
        if c["status"] == "zero_quote" {
            assert!(!confirmed);
            assert_eq!(number(&c["quote"]["returnData"]), U256::ZERO);
            amounts.1 += 1;
            continue;
        }
        assert!(confirmed);
        assert_eq!(c["status"], "filled");
        let r = &c["results"];
        let n = |k: &str| number(&r[k]["returnData"]);
        assert_eq!(n("before_in") - n("after_in"), number(&c["amount"]));
        assert_eq!(n("after_out") - n("before_out"), n("quote"));
        assert_eq!(n("after_vault_in") - n("vault_in"), number(&c["amount"]));
        assert_eq!(n("vault_out") - n("after_vault_out"), n("quote"));
        assert_eq!(r["exact"]["returnData"], "0x");
        assert_eq!(r["above"]["status"], "0x0");
        assert!(elfomo::within_capacity(n("quote"), n("vault_out")));
        amounts.0 += 1;
    }
    assert_eq!(amounts, (8, 4));
    let cap = &data["cap_probe"];
    assert_eq!(number(&cap["out"]), number(&cap["vault_balance"]));
    assert!(!elfomo::within_capacity(
        number(&cap["out"]),
        number(&cap["vault_balance"])
    ));
}
