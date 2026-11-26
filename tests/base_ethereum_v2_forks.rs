use alloy_primitives::U256;
use arb::venues::bsc_dex::math::v2::{amount_out, fee_ratio};
use rust_decimal::Decimal;
use serde_json::Value;
#[path = "common/first_run_profiles.rs"]
mod first_run_profiles;
mod native_common;

#[tokio::test]
async fn arbitrary_ring_id_reads_its_own_settings_and_requires_archive_proof_before_spend() {
    use arb::settings::scope::Scope;
    use arb::venues::bsc_dex::{ctx::Place, pools::ring};
    let mut ctx = native_common::ctx("ring-active-profile").await;
    ctx.place = Place {
        id: "eth_dex",
        all: &["eth_dex"],
    };
    let id = "user-ring@ethereum";
    let venue = Scope::Venue("eth_dex".to_string());
    let dex = Scope::Dex(id.into());
    let row = first_run_profiles::named(1, "ring-exchange-ethereum").unwrap();
    ctx.settings
        .set(
            "dex.evm_chain_id",
            venue.clone(),
            Some(serde_json::json!(1)),
            "test",
        )
        .await
        .unwrap();
    ctx.settings
        .set(
            "dex.native_wrapped",
            venue.clone(),
            Some(row["venue_settings"]["dex.native_wrapped"].clone()),
            "test",
        )
        .await
        .unwrap();
    ctx.settings
        .set(
            "dex.multicall3",
            venue,
            Some(row["venue_settings"]["dex.multicall3"].clone()),
            "test",
        )
        .await
        .unwrap();
    for (key, value) in row["settings"].as_object().unwrap() {
        ctx.settings
            .set(key, dex.clone(), Some(value.clone()), "test")
            .await
            .unwrap();
    }
    let original = ring::active(&ctx, id).unwrap();
    assert!(ring::checked(&ctx, id, original.factory).is_err());
    ctx.settings
        .set(
            "dex.few_factory",
            dex.clone(),
            Some(serde_json::json!(
                "0x0000000000000000000000000000000000001234"
            )),
            "test",
        )
        .await
        .unwrap();
    let moved = ring::active(&ctx, id).unwrap();
    assert_ne!(moved.few, original.few);
    assert!(ring::checked(&ctx, id, original.factory).is_err());
    assert!(ring::active(&ctx, "other-ring").is_err());
    for key in [
        "dex.few_factory",
        "dex.pool_code_hashes",
        "dex.pair_runtime_hash",
        "dex.ring_direct_assets",
    ] {
        ctx.settings
            .set(key, dex.clone(), None, "test")
            .await
            .unwrap();
        assert!(ring::active(&ctx, id).is_err());
        ctx.settings
            .set(key, dex.clone(), Some(row["settings"][key].clone()), "test")
            .await
            .unwrap();
    }
    ctx.settings
        .set(
            "dex.few_runtime_hash",
            dex,
            Some(row["settings"]["dex.pair_runtime_hash"].clone()),
            "test",
        )
        .await
        .unwrap();
    assert!(ring::active(&ctx, id).is_err());
}

fn uint(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn configured_v2_rates_match_the_pair_executor_and_wallet_balances() {
    let mut captures: Vec<Value> =
        serde_json::from_str(include_str!("data/base_ethereum_v2_probes.json")).unwrap();
    captures.extend(
        serde_json::from_str::<Vec<Value>>(include_str!(
            "data/base_ethereum_v2_variable_fees.json"
        ))
        .unwrap(),
    );
    let mut factories: serde_json::Map<String, Value> = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/ethereum_v2_factories.json"
    ))
    .unwrap();
    factories.extend(
        serde_json::from_str::<serde_json::Map<String, Value>>(include_str!(
            "data/first_run_dex_2026-11-26/base_v2_factories.json"
        ))
        .unwrap(),
    );
    let mut pool_count = 0;
    for (id, market) in &factories {
        let factory = market["factory"].as_str().unwrap().parse().unwrap();
        let init = market["origin"]["creation_code_hash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        for pool in market["pools"].as_array().unwrap() {
            let token = |i: usize| pool["tokens"][i].as_str().unwrap().parse().unwrap();
            let actual =
                arb::venues::bsc_dex::pools::verify::v2_create2(factory, token(0), token(1), init);
            assert_eq!(
                actual,
                pool["pool"]
                    .as_str()
                    .unwrap()
                    .parse::<alloy_primitives::Address>()
                    .unwrap()
            );
            pool_count += 1;
        }
        let sample = market["samples"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["cases"].is_array());
        let Some(sample) = sample else {
            assert_eq!(id, "bakeryswap-base");
            assert!(
                market["samples"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|s| s["reason"].is_string())
            );
            continue;
        };
        assert_eq!(sample["dex"], id.as_str());
        assert_eq!(sample["block_hash"], market["selector"]["blockHash"]);
        captures.push(sample.clone());
    }
    assert_eq!(pool_count, 781);
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let mut admitted = 0;
    let mut verified_boundaries = std::collections::BTreeSet::new();
    for profile in profiles.iter().filter(|p| {
        p["source"].as_str().is_some_and(|s| {
            s.contains("base_ethereum_v2_probes.json")
                || s.contains("base_ethereum_v2_variable_fees.json")
                || s.contains("ethereum_v2_factories.json")
                || s.contains("base_v2_factories.json")
        })
    }) {
        let capture = captures
            .iter()
            .find(|row| row["dex"] == profile["id"] && row["network"] == profile["network"])
            .unwrap();
        assert_eq!(profile["settings"]["dex.factory"], capture["factory"]);
        let live = profile["settings"]["dex.kind"] == "v2_fork_executor_create2_live_fee";
        if live || profile["settings"]["dex.kind"] == "v2_fork_executor_create2" {
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
        if !factories.contains_key(profile["id"].as_str().unwrap()) {
            assert_eq!(capture["runtime_match"], true);
        }
        let settings = &profile["settings"];
        let pct: Decimal = if live {
            use arb::venues::bsc_dex::pools::verify::v2_fee_pips;
            assert_eq!(capture["fee_getter"], settings["dex.v2_fee_method"]);
            let source = settings["dex.v2_fee_source"].as_str().unwrap();
            assert_eq!(capture["fee_target"], capture[source]);
            assert_eq!(
                settings["dex.v2_fee_denominator"],
                capture["fee_fraction"][0]
            );
            let denominator = capture["fee_fraction"][0].as_u64().unwrap();
            let value = capture["fee_fraction"][1].as_u64().unwrap();
            let pips = v2_fee_pips(U256::from(value), denominator).unwrap();
            assert_eq!(capture["pips"], pips);
            Decimal::new(i64::from(pips), 4)
        } else {
            assert_eq!(settings["dex.v2_fee_source"], "fixed");
            settings["dex.v2_fee_pct"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap()
        };
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
            .position(|t| t == capture["funded_asset"].as_str().unwrap_or(weth))
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
            if capture["verify_pair_boundary"] == true {
                for (direction, expected) in [("buy", bought), ("sell", returned)] {
                    let observed = &trade["pair_boundary"][direction];
                    assert_eq!(uint(&observed["max"]), expected);
                    let rejection = observed["next_revert"].as_str().unwrap();
                    match capture["invariant_error"].as_str() {
                        Some(expected) => assert_eq!(rejection, expected),
                        None => assert!(rejection.ends_with(": K")),
                    }
                }
            }
            for key in ["strict_buy", "strict_sell"] {
                assert!(trade[key]["code"].is_number());
            }
        }
        if capture["verify_pair_boundary"] == true {
            verified_boundaries.insert(profile["id"].as_str().unwrap());
        }
        admitted += 1;
    }
    for dex in [
        "apeswap_ethereum",
        "elk_finance_ethereum",
        "orion-ethereum",
        "saitaswap-ethereum",
        "sakeswap",
        "squadswap-base",
        "radioshack_ethereum",
        "degenbrains-base",
        "defi_swap",
        "fwx-base",
        "shibaswap",
        "torus",
        "dooar_ethereum",
        "standard_ethereum",
        "justmoney-ethereum",
        "verse",
        "spice_trade_ethereum",
        "9mm-v2-base",
        "cbswap",
        "cloudbase",
        "icecreamswap-base",
        "lfgswap-base",
        "robots-farm-base",
        "treble-v2",
    ] {
        assert!(
            verified_boundaries.contains(dex),
            "missing pair proof for {dex}"
        );
    }
    assert!(admitted >= 25);
}
