//! Проверка фактического размещения LunarBase и платежей в ETH/USDC по архиву контрольного блока.
use alloy_primitives::{Address, U256};
use arb::venues::bsc_dex::pools::lunar;
use serde_json::Value;

fn amount(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn primary_module_matches_all_markets_and_settles_full_wallet_deltas() {
    use arb::venues::bsc_dex::pools::direct_module;
    for (kind, raw, count) in [
        (
            "elfomofi",
            include_str!("data/first_run_dex_2026-11-26/base_pmm_executor_elfomo.json"),
            8,
        ),
        (
            "baibai",
            include_str!("data/first_run_dex_2026-11-26/base_pmm_executor_baibai.json"),
            8,
        ),
        (
            "lunarbase",
            include_str!("data/first_run_dex_2026-11-26/base_pmm_executor_lunar.json"),
            4,
        ),
    ] {
        let sample: Value = serde_json::from_str(raw).unwrap();
        let expected = sample["runtime_keccak"].as_str().unwrap().parse().unwrap();
        assert_eq!(direct_module::accepted(8453, kind), Some(expected));
        assert!(!direct_module::accepts(1, kind, expected));
        let cases = sample["cases"].as_array().unwrap();
        assert_eq!(cases.len(), count);
        for case in cases {
            let r = &case["results"];
            let n = |key: &str| amount(&r[key]["returnData"]);
            assert_eq!(
                n("wallet_in") - n("wallet_in_after"),
                amount(&case["amount"])
            );
            assert_eq!(n("wallet_out_after") - n("wallet_out"), n("quote"));
            assert_eq!(n("execute"), n("quote"));
            for k in [
                "wallet_allowance",
                "module_allowance",
                "module_in",
                "module_out",
            ] {
                assert_eq!(n(k), U256::ZERO);
            }
            for k in ["expired", "above", "wrong_market", "wrong_token", "repeat"] {
                assert_eq!(r[k]["status"], "0x0");
            }
            if kind == "lunarbase" {
                assert_eq!(n("native_after"), n("native_before"));
            }
        }
    }
}

#[test]
fn only_one_proxy_is_open_and_native_wallet_deltas_match_the_quote() {
    let d: Value =
        serde_json::from_str(include_str!("data/first_run_dex_2026-11-26/lunarbase.json")).unwrap();
    let mut live = Vec::new();
    for market in d["markets"].as_array().unwrap() {
        let view = |name: &str| {
            let row = market["views"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["signature"] == name)
                .unwrap();
            amount(&row["response"]["result"])
        };
        if view("paused()").is_zero() && view("isFresh()") == U256::ONE {
            live.push(market);
        }
    }
    assert_eq!(live.len(), 1);
    assert_eq!(
        live[0]["address"]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap(),
        lunar::POOL
    );
    assert_ne!(
        live[0]["implementation"]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap(),
        d["official"]["implementation"]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap()
    );
    let cases = d["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for c in cases {
        let native_in = c["token_in"]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap()
            .is_zero();
        let (a, b) = if native_in {
            ("eth", "usdc")
        } else {
            ("usdc", "eth")
        };
        let r = &c["results"];
        let balance = |asset: &str, when: &str| amount(&r[format!("{asset}_{when}")]["returnData"]);
        assert_eq!(
            balance(a, "before") - balance(a, "after"),
            amount(&c["amount"])
        );
        assert_eq!(
            balance(b, "after") - balance(b, "before"),
            amount(&c["output"])
        );
        assert_eq!(
            amount(&r["exact"]["returnData"]),
            amount(&r["quote"]["returnData"])
        );
        assert_eq!(r["expired"]["status"], "0x0");
        assert_eq!(r["above"]["status"], "0x0");
    }
}
