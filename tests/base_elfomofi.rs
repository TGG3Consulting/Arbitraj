//! Файл: tests/base_elfomofi.rs. Проверяет ABI реестра, границу vault и полные дельты с канонического Base.
use alloy_primitives::{Address, U256};
use arb::venues::bsc_dex::pools::elfomo;
use serde_json::Value;
mod native_common;

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

#[test]
fn module_acceptance_is_base_specific_and_bound_to_actual_wallet_deltas() {
    use arb::venues::bsc_dex::pools::{direct_module, families};
    let archive: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/elfomo_executor_base.json"
    ))
    .unwrap();
    let expected = archive["runtime_keccak"].as_str().unwrap().parse().unwrap();
    assert!(direct_module::accepts(8453, "elfomofi", expected));
    assert!(!direct_module::accepts(8453, "lunarbase", expected));
    assert_eq!(direct_module::accepted(1, "elfomofi"), None);
    let signal = families::make("elfomofi", "elfomofi").unwrap();
    let trade = families::make("elfomofi", "elfomofi_module").unwrap();
    assert!(!signal.tradable && trade.tradable && trade.direct_module());
    assert!(trade.executor_kind.is_none());
    let cases = archive["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 8);
    for case in cases {
        let r = &case["results"];
        let n = |key: &str| {
            r[key]["returnData"]
                .as_str()
                .unwrap()
                .parse::<U256>()
                .unwrap()
        };
        let amount = case["amount"].as_str().unwrap().parse::<U256>().unwrap();
        let out = case["out"].as_str().unwrap().parse::<U256>().unwrap();
        assert_eq!(n("wallet_in") - n("wallet_in_after"), amount);
        assert_eq!(n("wallet_out_after") - n("wallet_out"), out);
        assert_eq!((n("execute"), n("quote")), (out, out));
        for key in [
            "module_in",
            "module_out",
            "module_allowance",
            "wallet_allowance",
        ] {
            assert_eq!(n(key), U256::ZERO);
        }
        for key in ["expired", "above", "wrong_token", "wrong_market", "repeat"] {
            assert_eq!(r[key]["status"], "0x0");
        }
    }
    assert_eq!(archive["saturated"]["status"], "0x0");
}

#[test]
fn one_sided_depth_stays_in_the_graph_and_markets_share_one_vault() {
    use arb::core::types::{LadderPoint, OraclePmmData, PoolRef, PoolState};
    use arb::venues::bsc_dex::quote::{emptiness, spot};
    let mut e = native_common::entry("elfomofi", None);
    let contract = Address::repeat_byte(1);
    e.pool.reference = PoolRef::Member {
        contract,
        coin: e.coin,
    };
    let site = e.liquidity_site();
    e.pool.reference = PoolRef::Member {
        contract,
        coin: Address::repeat_byte(2),
    };
    assert_eq!(e.liquidity_site(), site);
    assert_eq!(site, PoolRef::Contract(contract));
    for buy in [true, false] {
        let (mut purchases, mut sales) = (Vec::new(), Vec::new());
        if buy {
            purchases.push(LadderPoint {
                amount_in: U256::from(6),
                amount_out: U256::from(2),
            });
        } else {
            sales.push(LadderPoint {
                amount_in: U256::from(2),
                amount_out: U256::from(6),
            });
        }
        e.pool.state = PoolState::OraclePmm(Box::new(OraclePmmData {
            price: U256::ZERO,
            spread: U256::ZERO,
            coeff: U256::ZERO,
            feasible: true,
            paused: false,
            implementation: Address::ZERO,
            base_is_token0: true,
            pool_token0: None,
            ladder_quote_in: purchases,
            ladder_base_in: sales,
        }));
        assert!(!emptiness::empty(&e));
        assert_eq!(spot::coins_for(&e, U256::from(6)), Some(U256::from(2)));
    }
}
