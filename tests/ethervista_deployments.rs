use alloy_primitives::{Address, B256, U256};
use arb::venues::bsc_dex::pools::verify::v2_create2;
use serde_json::Value;

#[test]
fn fees_and_active_router_match_every_registered_market_and_archived_wallet() {
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let docs = [
        (
            "base",
            "ethervista-base",
            91,
            include_str!("data/first_run_dex_2026-11-26/ethervista_base.json"),
        ),
        (
            "ethereum",
            "ethervista",
            1087,
            include_str!("data/first_run_dex_2026-11-26/ethervista_ethereum.json"),
        ),
    ];
    for (network, id, count, raw) in docs {
        let j: Value = serde_json::from_str(raw).unwrap();
        let p = &profiles
            .iter()
            .find(|p| p["network"] == network && p["id"] == id)
            .unwrap()["settings"];
        assert_eq!(p["dex.kind"], "ethervista_native");
        assert!(p.get("dex.router").is_none());
        assert_eq!(p["dex.quoter"], j["router"]);
        assert_eq!(p["dex.factory"], j["factory"]);
        assert_eq!(p["dex.pool_code_hashes"][0], j["init"]);
        assert_eq!(j["previous_router_factory"], j["factory"]);
        assert_ne!(j["previous_router"], j["router"]);
        for (field, hash) in [
            ("dex.factory", "dex.factory_runtime_hash"),
            ("dex.quoter", "dex.router_runtime_hash"),
        ] {
            assert_eq!(
                p[hash],
                j["verified"][p[field].as_str().unwrap()]["runtime_hash"]
            );
        }
        let init = j["init"].as_str().unwrap().parse::<B256>().unwrap();
        let factory = j["factory"].as_str().unwrap().parse::<Address>().unwrap();
        let rows = j["registry"].as_array().unwrap();
        assert_eq!(rows.len(), count);
        for market in rows {
            let t = |i: usize| market["tokens"][i].as_str().unwrap().parse().unwrap();
            assert_eq!(
                v2_create2(factory, t(0), t(1), init),
                market["pool"].as_str().unwrap().parse::<Address>().unwrap()
            );
            for units in market["fee_units"].as_array().unwrap() {
                let native = j["fee_wei_per_units"][units.to_string()]
                    .as_str()
                    .unwrap()
                    .parse::<U256>()
                    .unwrap();
                assert_eq!(native.is_zero(), units.as_u64() == Some(0));
            }
        }
        let mut defects = [0, 0, 0];
        for market in rows {
            for k in 0..2 {
                let Some(held) = market["balances"][k].as_str() else {
                    defects[0] += 1;
                    continue;
                };
                let held: U256 = held.parse().unwrap();
                let reserve: U256 = market["reserves"][k].as_str().unwrap().parse().unwrap();
                defects[1] += usize::from(held < reserve);
                defects[2] += usize::from(held > reserve);
            }
        }
        assert_eq!(defects, if network == "base" { [0, 0, 0] } else { [1, 1, 1] });
        let observed = &j["observations"][0];
        let pool = rows.iter().find(|p| p["pool"] == observed["pool"]).unwrap();
        for (i, side) in ["buy", "sell"].into_iter().enumerate() {
            let units = observed["fees"][side]["units"].as_u64().unwrap();
            assert_eq!(pool["fee_units"][i], units);
            assert_eq!(
                j["fee_wei_per_units"][units.to_string()],
                observed["fees"][side]["native_wei"]
            );
        }
        let cases = observed["swaps"].as_array().unwrap();
        assert_eq!(cases.len(), 2);
        for c in cases {
            assert_eq!(c["buy_at_measured"]["status"], "0x1");
            assert_eq!(c["sell_same_wallet"]["status"], "0x1");
            assert_eq!(c["previous_router_same_buy"]["status"], "0x0");
            assert_eq!(c["buy_min_plus_one"]["status"], "0x0");
            assert_eq!(c["sell_min_plus_one"]["status"], "0x0");
            assert_eq!(c["sell_fee_minus_one"]["status"], "0x0");
            assert_eq!(c["sell_same_wallet"]["router_weth_before"], "0");
        }
    }
}
