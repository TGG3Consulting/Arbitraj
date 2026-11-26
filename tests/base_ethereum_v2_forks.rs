use alloy_primitives::U256;
use arb::venues::bsc_dex::math::v2::{amount_out, fee_ratio};
use rust_decimal::Decimal;
use serde_json::Value;

fn uint(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn fixed_fee_profiles_match_the_executor_and_wallet_balances() {
    let captures: Vec<Value> =
        serde_json::from_str(include_str!("data/base_ethereum_v2_probes.json")).unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let mut admitted = 0;
    for profile in profiles.iter().filter(|p| {
        p["source"]
            .as_str()
            .is_some_and(|s| s.contains("base_ethereum_v2_probes.json"))
    }) {
        let capture = captures
            .iter()
            .find(|row| row["dex"] == profile["id"] && row["network"] == profile["network"])
            .unwrap();
        assert_eq!(profile["settings"]["dex.factory"], capture["factory"]);
        if profile["settings"]["dex.kind"] == "v2_fork_executor_create2" {
            use alloy_primitives::{Address, B256};
            use arb::venues::bsc_dex::pools::verify::v2_create2;
            let parse = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
            let hash: B256 = capture["creation_code_hash"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(
                profile["settings"]["dex.pool_code_hashes"][0],
                capture["creation_code_hash"]
            );
            let (a, b) = (parse(&capture["tokens"][0]), parse(&capture["tokens"][1]));
            let factory = parse(&capture["factory"]);
            let pool = parse(&capture["pool"]);
            assert_eq!(v2_create2(factory, a, b, hash), pool);
            assert_eq!(v2_create2(factory, b, a, hash), pool);
            assert_ne!(v2_create2(factory, a, b, B256::ZERO), pool);
        } else {
            assert_eq!(capture["factory_proof"]["embedded_pair"], true);
        }
        assert_eq!(capture["runtime_match"], true);
        assert_eq!(profile["settings"]["dex.v2_fee_source"], "fixed");
        let pct: Decimal = profile["settings"]["dex.v2_fee_pct"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let fee = fee_ratio(pct).unwrap();
        let weth = match profile["chain_id"].as_u64().unwrap() {
            8453 => "0x4200000000000000000000000000000000000006",
            1 => "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2",
            chain => panic!("unexpected chain {chain}"),
        };
        let i = capture["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .position(|t| t == weth)
            .unwrap();
        let input_reserve = uint(&capture["reserves"][i]);
        let output_reserve = uint(&capture["reserves"][1 - i]);
        let cases = capture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 2);
        assert!(uint(&cases[0]["in"]) < uint(&cases[1]["in"]));
        for trade in cases {
            let input = uint(&trade["in"]);
            let bought = amount_out(input, input_reserve, output_reserve, fee).unwrap();
            let returned =
                amount_out(bought, output_reserve - bought, input_reserve + input, fee).unwrap();
            assert_eq!(bought, uint(&trade["buy_return"]));
            assert_eq!(returned, uint(&trade["sell_return"]));
            let balances = trade["observed_balances"]
                .as_array()
                .unwrap()
                .iter()
                .map(uint)
                .collect::<Vec<_>>();
            assert_eq!(
                balances,
                [bought, returned, U256::ZERO, U256::ZERO, U256::ZERO]
            );
            for key in ["strict_buy", "strict_sell"] {
                assert!(trade[key]["code"].is_number());
            }
        }
        admitted += 1;
    }
    assert!(admitted >= 14);
}
