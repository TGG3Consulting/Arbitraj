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
    let archived: serde_json::Map<String, Value> = serde_json::from_reader(
        flate2::read::GzDecoder::new(include_bytes!(
            "data/first_run_dex_2026-11-26/llama_eth_v2_wallets.json.gz"
        ).as_slice()),
    ).unwrap();
    assert_eq!(archived.len(), 8);
    for (slug, book) in archived {
        assert!(factories.insert(format!("{slug}-ethereum"), book).is_none());
    }
    let base_archived: serde_json::Map<String, Value> = serde_json::from_reader(
        flate2::read::GzDecoder::new(include_bytes!(
            "data/first_run_dex_2026-11-26/llama_base_v2_wallets.json.gz"
        ).as_slice()),
    ).unwrap();
    assert_eq!(base_archived.len(), 3);
    for (slug, book) in base_archived {
        assert!(factories.insert(format!("{slug}-base"), book).is_none());
    }
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
            let router = &market["native_router"];
            let admitted = router["samples"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["cases"].is_array())
                .unwrap();
            let pair = market["pools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["pool"] == admitted["pool"])
                .unwrap();
            let w = pair["tokens"]
                .as_array()
                .unwrap()
                .iter()
                .position(|t| t == "0x4200000000000000000000000000000000000006")
                .unwrap();
            let (r0, r1) = (uint(&pair["reserves"][w]), uint(&pair["reserves"][1 - w]));
            let fee = fee_ratio(Decimal::new(3, 1)).unwrap();
            let cases = admitted["cases"].as_array().unwrap();
            assert_eq!(cases.len(), 2);
            for c in cases {
                let input = uint(&c["buy"]["spent"]);
                let bought = amount_out(input, r0, r1, fee).unwrap();
                assert_eq!(bought, uint(&c["buy"]["received"]));
                assert_eq!(bought, uint(&c["sell"]["spent"]));
                assert_eq!(
                    amount_out(bought, r1 - bought, r0 + input, fee).unwrap(),
                    uint(&c["sell"]["received"])
                );
                for side in ["buy", "sell"] {
                    assert_eq!(c[side]["min_plus_one"]["code"], 3);
                }
            }
            continue;
        };
        assert_eq!(sample["dex"], id.as_str());
        assert_eq!(sample["block_hash"], market["selector"]["blockHash"]);
        captures.push(sample.clone());
    }
    assert_eq!(pool_count, 1519);
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let mut admitted = 0;
    let mut verified_boundaries = std::collections::BTreeSet::new();
    for profile in profiles.iter().filter(|p| {
        p["source"].as_str().is_some_and(|s| {
            s.contains("base_ethereum_v2_probes.json")
                || s.contains("base_ethereum_v2_variable_fees.json")
                || s.contains("ethereum_v2_factories.json")
                || s.contains("llama_eth_v2_wallets.json.gz")
                || s.contains("llama_base_v2_wallets.json.gz")
                || s.contains("base_v2_factories.json")
        })
    }) {
        if profile["settings"]["dex.kind"] == "v2_fork_router_create2" {
            let row = &factories[profile["id"].as_str().unwrap()];
            assert_eq!(
                profile["settings"]["dex.router"],
                row["native_router"]["router"]
            );
            assert_eq!(profile["settings"]["dex.factory"], row["factory"]);
            assert_eq!(
                profile["settings"]["dex.pool_code_hashes"][0],
                row["origin"]["creation_code_hash"]
            );
            assert_eq!(profile["settings"]["dex.v2_fee_pct"], "0.3");
            assert_eq!(
                arb::venues::bsc_dex::pools::exec_kind::of("v2_fork_router_create2"),
                None
            );
            continue;
        }
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
        "elk-base",
        "kinetix-v2-base",
        "velox-base",
        "bxh-ethereum",
        "empire-ethereum",
        "luaswap-ethereum",
        "sumswap-ethereum",
        "swapsicle-ethereum",
        "wswap-ethereum",
        "yoshi-ethereum",
        "youswap-ethereum",
        "dooar_ethereum",
        "standard_ethereum",
        "justmoney-ethereum",
        "verse",
        "spice_trade_ethereum",
        "9mm-v2-base",
        "moonbase",
        "x7-finance-base",
        "x7-finance",
        "x7-finance-ethereum",
        "9inch-ethereum",
        "cbswap",
        "cloudbase",
        "pixelswap-base",
        "satori-base",
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

#[test]
fn bluedex_registered_pair_and_router_settle_at_the_independent_curve_output() {
    use alloy_primitives::{Address, B256};
    use arb::venues::bsc_dex::pools::verify::v2_create2;
    let market: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/bluedex_base.json"
    ))
    .unwrap();
    let profile = first_run_profiles::named(8453, "bluedex").unwrap();
    let settings = &profile["settings"];
    assert_eq!(settings["dex.kind"], "v2_fork_dual_create2");
    assert_eq!(settings["dex.router"], market["native_router"]["router"]);
    assert_eq!(
        settings["dex.router_runtime_hash"],
        market["native_router"]["runtime"]
    );
    assert_eq!(
        settings["dex.pool_code_hashes"][0],
        market["origin"]["creation_code_hash"]
    );
    let factory: Address = settings["dex.factory"].as_str().unwrap().parse().unwrap();
    let init: B256 = settings["dex.pool_code_hashes"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let registry = market["pools"].as_array().unwrap();
    assert_eq!(registry.len(), 4);
    for p in registry {
        let [a, b]: [Address; 2] = serde_json::from_value(p["tokens"].clone()).unwrap();
        let pool: Address = serde_json::from_value(p["pool"].clone()).unwrap();
        assert_eq!(v2_create2(factory, a, b, init), pool);
        assert_eq!(v2_create2(factory, b, a, init), pool);
    }
    let fee = fee_ratio(
        settings["dex.v2_fee_pct"]
            .as_str()
            .unwrap()
            .parse::<Decimal>()
            .unwrap(),
    )
    .unwrap();
    let mut matched = 0;
    for via in market["native_router"]["samples"].as_array().unwrap() {
        let Some(rows) = via["cases"].as_array() else {
            continue;
        };
        let sample = market["samples"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["pool"] == via["pool"] && s["cases"].is_array())
            .unwrap();
        let weth = "0x4200000000000000000000000000000000000006";
        let i = usize::from(sample["tokens"][1] == weth);
        let rin = uint(&sample["reserves"][i]);
        let rout = uint(&sample["reserves"][1 - i]);
        for row in rows {
            let buy = &row["buy"];
            let sell = &row["sell"];
            let n = uint(&buy["amount"]);
            let out = amount_out(n, rin, rout, fee).unwrap();
            let back = amount_out(out, rout - out, rin + n, fee).unwrap();
            assert_eq!(
                (out, back),
                (uint(&buy["received"]), uint(&sell["received"]))
            );
            for leg in [buy, sell] {
                assert_eq!(leg["amount"], leg["spent"]);
                assert_eq!(leg["received"], leg["quoted"]);
                assert!(leg["min_plus_one"].is_object());
            }
            let independent = sample["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| uint(&c["in"]) == n)
                .unwrap();
            assert_eq!(
                (out, back),
                (
                    uint(&independent["buy_return"]),
                    uint(&independent["sell_return"])
                )
            );
            matched += 1;
        }
    }
    assert!(matched >= 2);
}

#[test]
fn rabbit_router_wallets_match_the_two_fee_floors_in_both_directions() {
    let market: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/bunnyswap_base.json"
    ))
    .unwrap();
    let profile = first_run_profiles::named(8453, "bunnyswap").unwrap();
    let s = &profile["settings"];
    assert_eq!(s["dex.bunny_pair"], market["pair"]);
    assert_eq!(s["dex.quoter"], market["router"]);
    assert_eq!(
        s["dex.quoter_runtime_hash"],
        market["hashes"][market["router"].as_str().unwrap()]
    );
    assert_eq!(
        s["dex.pair_runtime_hash"],
        market["hashes"][market["pair"].as_str().unwrap()]
    );
    let rows = market["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for row in rows {
        for (side, buy) in [("buy", true), ("sell", false)] {
            let leg = &row[side];
            let fee = leg["lp"].as_u64().unwrap();
            let protocol = U256::from(leg["protocol"].as_u64().unwrap());
            let n = uint(&leg["amount"]);
            let i = usize::from(buy);
            let net = if buy {
                n - n * protocol / U256::from(1000)
            } else {
                n
            };
            let result = amount_out(
                net,
                uint(&leg["reserves"][i]),
                uint(&leg["reserves"][1 - i]),
                fee_ratio(Decimal::new(fee as i64, 1)).unwrap(),
            )
            .unwrap();
            assert_eq!(result, uint(&leg["gross"]));
            assert_eq!(
                if buy {
                    result
                } else {
                    result - result * protocol / U256::from(1000)
                },
                uint(&leg["received"])
            );
            assert!(leg["min_plus_one"].is_object() && leg["expired"].is_object());
        }
        assert_eq!(row["buy"]["received"], row["sell"]["amount"]);
    }
}

#[test]
fn rabbit_module_and_v5_pay_the_same_wallet_amounts_as_the_official_router() {
    let native: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/bunnyswap_base.json"
    ))
    .unwrap();
    let module: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/bunnyswap_base_module.json"
    ))
    .unwrap();
    let profile = first_run_profiles::named(8453, "bunnyswap").unwrap();
    assert_eq!(
        profile["settings"]["dex.module_runtime_hashes"][0],
        module["module_hash"]
    );
    assert_eq!(native["selector"], module["selector"]);
    assert_eq!(native["router"], module["router"]);
    let forms = module["cases"].as_array().unwrap();
    assert_eq!(forms.len(), 6);
    for row in native["cases"].as_array().unwrap() {
        for form in ["direct", "v5"] {
            let case = forms
                .iter()
                .find(|c| c["mode"] == form && c["buy"]["amount"] == row["buy"]["amount"])
                .unwrap();
            assert_eq!(case["pool"], native["pair"]);
            for side in ["buy", "sell"] {
                let settled = &case[side];
                assert_eq!(settled["amount"], row[side]["amount"]);
                assert_eq!(settled["received"], row[side]["received"]);
                assert_eq!(settled["strict"]["code"], 3);
                assert_eq!(settled["expired"]["code"], 3);
                assert_eq!(settled["before"][0], settled["amount"]);
                assert_eq!(settled["after"][1], settled["received"]);
                for index in [0, 2, 3, 4, 5, 6, 7] {
                    assert_eq!(settled["after"][index], "0");
                }
            }
        }
    }
}

#[test]
fn swapr_both_actual_pair_rates_settle_the_signers_own_assets_on_both_routes() {
    use arb::venues::bsc_dex::pools::families;
    use std::collections::BTreeSet;

    let archive: Value = serde_json::from_reader(flate2::read::GzDecoder::new(
        &include_bytes!("data/first_run_dex_2026-11-26/swapr_ethereum.json.gz")[..],
    ))
    .unwrap();
    let row = first_run_profiles::named(1, "swapr_ethereum").unwrap();
    let settings = &row["settings"];
    let family = families::make("user-swapr", settings["dex.kind"].as_str().unwrap()).unwrap();
    assert!(family.variant.dual_v2_execution() && family.variant.v2_block_state());
    let roles = &archive["roles"];
    assert_eq!(
        settings["dex.factory_runtime_hash"],
        roles["Factory"]["runtime"]
    );
    assert_eq!(settings["dex.pair_runtime_hash"], roles["Pair"]["runtime"]);
    assert_eq!(
        settings["dex.router_runtime_hash"],
        roles["Router"]["runtime"]
    );
    assert_eq!(
        settings["dex.pool_code_hashes"][0],
        archive["creation_code_hash"]
    );
    assert_eq!(settings["dex.v2_fee_source"], "pool");
    assert_eq!(settings["dex.v2_fee_method"], "swapFee()");
    assert_eq!(settings["dex.v2_fee_denominator"], 10000);
    let pairs = archive["markets"].as_array().unwrap();
    assert_eq!(pairs.len(), 44);
    assert_eq!(pairs.iter().filter(|m| m["fee_bps"] == 25).count(), 43);
    assert_eq!(pairs.iter().filter(|m| m["fee_bps"] == 50).count(), 1);
    let mut observed = BTreeSet::new();
    for e in archive["examples"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e.get("wallets").is_some())
    {
        let bps = e["fee_bps"].as_i64().unwrap();
        let fee = fee_ratio(Decimal::new(bps, 2)).unwrap();
        let m = pairs.iter().find(|m| m["pool"] == e["pool"]).unwrap();
        assert_eq!(m["balances"], m["reserves"]);
        for case in e["wallets"].as_array().unwrap() {
            let buy = &case["buy"];
            let sell = &case["sell"];
            let i = usize::from(m["tokens"][0] != buy["input"]);
            let rin = uint(&m["reserves"][i]);
            let rout = uint(&m["reserves"][1 - i]);
            let input = uint(&case["size"]);
            let out = amount_out(input, rin, rout, fee).unwrap();
            let back = amount_out(out, rout - out, rin + input, fee).unwrap();
            for (leg, expected_spend, expected_credit, side) in
                [(buy, input, out, "buy"), (sell, out, back, "sell")]
            {
                assert_eq!(uint(&leg["exact"]["spent"]), expected_spend);
                assert_eq!(uint(&leg["exact"]["received"]), expected_credit);
                assert_eq!(leg["above"]["status"], "0x0");
                assert_eq!(uint(&leg["above"]["spent"]), U256::ZERO);
                assert_eq!(uint(&leg["above"]["received"]), U256::ZERO);
                assert!(observed.insert((bps, case["mode"].as_str().unwrap(), input, side)));
            }
        }
    }
    assert_eq!(observed.len(), 16);
}
