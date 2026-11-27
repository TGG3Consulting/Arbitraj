//! Привязывает роли Relay и котируемую пару к фактическим данным кошелька Base.
use alloy_primitives::{B256, Bytes, keccak256};
use serde_json::Value;

fn unpack(data: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(data)).unwrap()
}
#[test]
fn production_uses_the_routing_table_of_the_measured_baseline_market() {
    let source = unpack(include_bytes!(
        "data/first_run_dex_2026-11-26/baseline_base_components.json.gz"
    ));
    let wallet = unpack(include_bytes!(
        "data/first_run_dex_2026-11-26/baseline_base_wallets.json.gz"
    ));
    let cfg: Value = serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    let settings = &cfg
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "baseline-base" && p["chain_id"] == 8453)
        .unwrap()["settings"];
    assert_eq!(settings["dex.factory"], source["relay"]);
    assert_eq!(source["selector"], wallet["selector"]);
    assert_eq!(source["key"], wallet["markets"][0]["key"]);
    let key: Bytes = settings["dex.launch_pool_keys"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let id: B256 = source["pool_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(keccak256(key), id);
    assert_eq!(source["components"].as_object().unwrap().len(), 7);
    assert_eq!(source["routes"].as_object().unwrap().len(), 113);
    assert_eq!(source["flags"]["hasHook(address)"], true);
    assert_eq!(source["flags"]["isPoolPaused(address)"], false);
    assert_eq!(source["flags"]["isProtocolPaused()"], false);
    for role in settings["dex.launch_roles"].as_array().unwrap() {
        let r: Vec<_> = role.as_str().unwrap().split('@').collect();
        assert_eq!(source["contracts"][r[1]]["hash"], r[2]);
    }
    for (signature, component) in [
        (
            "beforeSwap(address,(address,address,uint24,int24,address),(bool,int256,uint160),bytes)",
            "BHook",
        ),
        (
            "afterSwap(address,(address,address,uint24,int24,address),(bool,int256,uint160),int256,bytes)",
            "BHook",
        ),
        ("buyTokensExactIn(address,uint256,uint256)", "BSwap"),
        ("sellTokensExactIn(address,uint256,uint256)", "BSwap"),
        ("poolIdToBToken(bytes32)", "BLens"),
    ] {
        let sel = alloy_primitives::hex::encode_prefixed(&keccak256(signature)[..4]);
        assert_eq!(source["routes"][sel], source["components"][component]);
    }
}
