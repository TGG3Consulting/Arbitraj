//! Bind the four paid LLAMMA bridge legs to the factory profile and the production Curve math.
use alloy_primitives::U256;
use arb::venues::bsc_dex::math::stableswap_v1::{Template, exchange_out_t};
use flate2::read::GzDecoder;
use serde_json::Value;

#[test]
fn both_cash_directions_match_the_original_pool_and_real_transfers() {
    let registry: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/crvusd_cash_ethereum.json"
    ))
    .unwrap();
    let archive: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!("data/first_run_dex_2026-11-26/llamma_ethereum_wallets.json.gz")[..],
    ))
    .unwrap();
    let profiles: Vec<Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let profile = &profiles
        .iter()
        .find(|p| p["id"] == "curve-crvusd-cash" && p["chain_id"] == 1)
        .unwrap()["settings"];
    assert_eq!(profile["dex.kind"], "curve_stableswap_plain");
    assert_eq!(profile["dex.factory"], registry["factory"]);
    assert_eq!(
        profile["dex.curve_plain_implementations"][0],
        registry["implementation"]
    );
    assert_eq!(profile["dex.stable_pools"][0], archive["bridge"]);
    assert_eq!(archive["selector"], registry["selector"]);
    assert_eq!(archive["bridge"], registry["pool"]);
    let value = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let template = Template {
        amp_precision: U256::from(100u8),
        max_rounds: 255,
        strict: true,
        pair_d: true,
    };
    let balances = registry["balances"]
        .as_array()
        .unwrap()
        .iter()
        .map(value)
        .collect::<Vec<_>>();
    let rates = registry["decimals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| U256::from(10u8).pow(U256::from(36 - d.as_u64().unwrap())))
        .collect::<Vec<_>>();
    let paid = archive["usdt"].as_array().unwrap();
    let reads = registry["quotes"].as_array().unwrap();
    assert_eq!(paid.len(), 4);
    assert_eq!(paid.len(), reads.len());
    for (payment, quote) in paid.iter().zip(reads) {
        let i = usize::from(payment["target"] == registry["coins"][0]);
        let spent = value(&payment[if i == 0 { "spent" } else { "intermediateSpent" }]);
        let net = value(
            &payment[if i == 0 {
                "intermediateReceived"
            } else {
                "received"
            }],
        );
        assert_eq!((value(&quote["dx"]), value(&quote["dy"])), (spent, net));
        assert_eq!(
            exchange_out_t(
                i,
                1 - i,
                spent,
                &balances,
                &rates,
                value(&registry["a"]),
                value(&registry["fee"]),
                &template
            ),
            Some(net)
        );
        assert_eq!(payment["changeCrvUSD"], "0");
    }
}
