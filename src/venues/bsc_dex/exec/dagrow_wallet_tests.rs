//! Публичный архив DAGrow. Сверяет подписанные calldata, реальные журналы, чистые суммы и прежнюю запись после рестарта.
use super::dagrow_wallet::{Proof, Terms};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;
#[test]
fn archived_direct_and_protected_recover_by_signed_limits_and_real_wallet_logs() {
    let d:Value=serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/dagrow_ethereum_protected_wallets.json.gz")[..])).unwrap();
    let addr = |v: &Value| v.as_str().unwrap().parse::<Address>().unwrap();
    let num = |v: &Value| v.as_str().unwrap().parse::<U256>().unwrap();
    let stable = addr(&d["cases"][0]["input"]);
    let mut checked = 0;
    for c in d["cases"].as_array().unwrap() {
        let mode = c["mode"].as_str().unwrap();
        if mode == "v5" {
            continue;
        }
        let raw = c["call"]["data"]
            .as_str()
            .unwrap()
            .parse::<Bytes>()
            .unwrap();
        let native = mode.starts_with("direct");
        let exact = mode.ends_with("out");
        let spent = num(&c["spent"]);
        let received = num(&c["received"]);
        let terms = Terms {
            pool: addr(&d["roles"]["pool"]["address"]),
            target: addr(&c["target"]),
            stable,
            desired: exact.then_some(received),
        };
        let j = 4 + 32 * if native { 4 } else { 5 };
        let p = Proof {
            terms,
            payer: addr(&d["owner"]),
            input: addr(&c["input"]),
            output: addr(&c["output"]),
            amount: spent,
            minimum: received,
            deadline: U256::from_be_slice(&raw[j..j + 32]),
        };
        let mut p: Proof = serde_json::from_value(serde_json::to_value(p).unwrap()).unwrap();
        assert_eq!(p.data(), raw);
        assert_eq!(p.payer, addr(&c["call"]["from"]));
        let logs = serde_json::from_value::<Vec<crate::venues::bsc_dex::check::simulate::SimLog>>(
            c["logs"].clone(),
        )
        .unwrap();
        assert_eq!(p.measured(&logs), Some((spent, received)));
        let mut ambiguous = logs.clone();
        ambiguous.extend(logs.clone());
        assert!(p.measured(&ambiguous).is_none());
        p.minimum += U256::ONE;
        assert!(p.measured(&logs).is_none());
        p.minimum = received;
        p.payer = p.terms.pool;
        assert!(p.measured(&logs).is_none());
        assert_eq!(c["failures"].as_array().unwrap().len(), 2);
        checked += 1;
    }
    assert_eq!(checked, 16);
}
