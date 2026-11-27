use super::{
    profile::{Profile, Role},
    receipt::Saved,
};
use crate::venues::bsc_dex::exec::signed_call::Erc20Call;
use alloy::rpc::types::Log;
use alloy_primitives::{Address, B256, U256};
use flate2::read::GzDecoder;
use serde_json::Value;
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn hash(v: &Value) -> B256 {
    v.as_str().unwrap().parse().unwrap()
}
fn amount(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn role(v: &Value) -> Role {
    Role {
        addr: addr(&v["address"]),
        hash: hash(&v["hash"]),
        proxy: v
            .get("implementation")
            .map(|i| (addr(&i["address"]), hash(&i["hash"]))),
    }
}
#[test]
fn historical_simple_and_partial_guarded_fills_recover_only_the_credited_net() {
    let load = |b: &[u8]| serde_json::from_reader::<_, Value>(GzDecoder::new(b)).unwrap();
    let markets = load(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/dexalot_base_registry.json.gz"
    ));
    let d = load(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/dexalot_base_wallet.json.gz"
    ));
    let w = &markets["witness"];
    let owner = addr(&d["owner"]);
    let module = addr(&d["fixture_module"]);
    let p = Profile {
        maker: role(&w["maker"]),
        router: role(&w["router"]),
        module: Some(Role {
            addr: module,
            hash: hash(&d["manifest"]["runtimeKeccak"]),
            proxy: None,
        }),
    };
    let input = addr(&w["input"]);
    let output = addr(&w["output"]);
    let nonce = amount(&w["nonce"]);
    let mut modes = Vec::new();
    for r in d["controls"].as_array().unwrap() {
        let partial = r["partial"].as_bool().unwrap();
        modes.push(partial);
        let nominal = amount(&r["nominal"]);
        let net = amount(&r["net"]);
        let spent = amount(&r["spent"]);
        assert!(net < nominal);
        let logs: Vec<Log> = serde_json::from_value(r["logs"].clone()).unwrap();
        let mut proof = Saved {
            call: Erc20Call {
                chain: 8453,
                payer: owner,
                module,
                calldata: B256::ZERO,
            },
            profile: p,
            nonce,
            digest: B256::ZERO,
            expiry: w["expiry"]
                .as_u64()
                .or_else(|| w["expiry"].as_str().and_then(|s| s.parse().ok()))
                .unwrap(),
            input,
            output,
            source: input,
            destination: output,
            taker: addr(&w["taker"]),
            payee: module,
            amount: spent,
            quoted: amount(&w["nominal"]),
            minimum: net,
            partial,
        };
        assert_eq!(proof.amounts(&logs), Some((spent, net)));
        proof.minimum = net + U256::ONE;
        assert_eq!(proof.amounts(&logs), None);
        proof.minimum = net;
        proof.nonce ^= U256::ONE;
        assert_eq!(proof.amounts(&logs), None);
    }
    assert_eq!(modes, [false, true]);
}
