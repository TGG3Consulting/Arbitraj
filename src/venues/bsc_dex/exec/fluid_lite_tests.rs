use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;
fn read(bytes: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
}
fn a(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn n(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
#[test]
fn two_physical_markets_and_twelve_paid_wallet_routes() {
    let raw = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/fluid_dex_lite_ethereum.json.gz"
    ));
    let wallet = read(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/fluid_dex_lite_wallet.json.gz"
    ));
    assert_eq!(wallet["head"], raw["active"]["head"]);
    let rows = raw["active"]["pools"].as_array().unwrap();
    let reference = raw["reference"]["pools"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.len(), reference.len());
    let owner = a(&wallet["owner"]);
    let dex = a(&wallet["manifest"]["exchange"]);
    let wrapped = a(&wallet["manifest"]["wrapped"]);
    let module = a(&wallet["module"]);
    let markets: Vec<_> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let market = Market {
                token0: a(&row["token0"]),
                token1: a(&row["token1"]),
                salt: row["salt"].as_str().unwrap().parse().unwrap(),
                index: U256::from(i),
            };
            let id = market.id().unwrap();
            assert_eq!(
                &id[..8],
                hex::decode(&row["pool_id"].as_str().unwrap()[2..])
                    .unwrap()
                    .as_slice()
            );
            assert_eq!(reference[i]["pool_id"], row["pool_id"]);
            assert!(
                reference[i]["payments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|v| v["funded_error"].is_object())
            );
            assert!(
                row["payments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|v| !n(&v["received"]).is_zero() && !n(&v["spent"]).is_zero())
            );
            market
        })
        .collect();
    let mut totals = [0; 2];
    for trade in wallet["payments"].as_array().unwrap() {
        let key = trade["market"].as_str().unwrap().parse::<B256>().unwrap();
        let market = *markets.iter().find(|m| m.id() == Some(key)).unwrap();
        let logs: Vec<Log> = serde_json::from_value(trade["logs"].clone()).unwrap();
        let input = a(&trade["source"]);
        let output = a(&trade["target"]);
        let spent = n(&trade["spent"]);
        let net = n(&trade["received"]);
        assert!(trade["strict"].is_object());
        assert!(!trade["controls"].as_array().unwrap().is_empty());
        if trade["mode"] == "direct" {
            let calldata = trade["tx"]["data"]
                .as_str()
                .unwrap()
                .parse::<Bytes>()
                .unwrap();
            let wire = abi::IWallet::executeCall::abi_decode(&calldata).unwrap();
            let proof = Proof {
                payer: owner,
                input,
                output,
                maximum: wire.budget,
                minimum: wire.minimum,
                deadline: wire.deadline,
                terms: Terms {
                    market,
                    target: module,
                    wrapped,
                    dex,
                    desired: wire.exactOut.then_some(wire.minimum),
                    module_hash: wallet["manifest"]["runtimeKeccak"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap(),
                },
            };
            assert_eq!(proof.data(), calldata);
            assert_eq!(
                proof.amounts(&logs, owner, input, output),
                Some((spent, net))
            );
            let mut wrong = proof.clone();
            wrong.terms.market.salt = B256::repeat_byte(0x81);
            assert!(wrong.amounts(&logs, owner, input, output).is_none());
            let mut duplicated = logs.clone();
            duplicated.push(
                logs.iter()
                    .find(|l| {
                        l.address() == dex
                            && l.topic0() == Some(&abi::IWallet::LogSwap::SIGNATURE_HASH)
                    })
                    .unwrap()
                    .clone(),
            );
            assert!(proof.amounts(&duplicated, owner, input, output).is_none());
            totals[0] += 1;
        } else {
            let event = logs
                .iter()
                .find(|l| {
                    l.address() == module
                        && l.topic0() == Some(&abi::IWallet::Filled::SIGNATURE_HASH)
                })
                .unwrap();
            let paid = abi::IWallet::Filled::decode_log_data(&event.inner.data).unwrap();
            assert_eq!(
                (
                    paid.market,
                    paid.payer,
                    paid.recipient,
                    paid.spent,
                    paid.received
                ),
                (key, a(&trade["tx"]["to"]), owner, spent, net)
            );
            totals[1] += 1;
        }
    }
    assert_eq!(totals, [8, 4]);
}
