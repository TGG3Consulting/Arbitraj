//! Both wallet entrypoints, per-recipient integer floors, and the reserves on emitted Sync.
use alloy_primitives::{Address, U256};
use arb::core::types::ToonState;
use serde_json::Value;
fn raw(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
#[test]
fn wallet_replay_and_minimum_debit_match_each_source() {
    let bytes = include_bytes!("data/first_run_dex_2026-11-26/toonswap_ethereum_wallets.json.gz");
    let d: Value = serde_json::from_reader(flate2::read::GzDecoder::new(bytes.as_slice())).unwrap();
    let markets = d["markets"].as_array().unwrap();
    assert_eq!(markets.len(), 11);
    let cases = d["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 80);
    for row in cases {
        let market = markets.iter().find(|m| m["pool"] == row["pool"]).unwrap();
        let reserves = [raw(&row["reserves"][0]), raw(&row["reserves"][1])];
        let s = ToonState {
            reserves,
            balances: reserves,
            pool: addr(&row["pool"]),
            recipients: [addr(&d["roles"]["dibs"]), addr(&d["roles"]["treasury"])],
            rates: [
                U256::from(d["rates"][0].as_u64().unwrap()),
                U256::from(d["rates"][1].as_u64().unwrap()),
            ],
        };
        let zfo = row["source"] == market["tokens"][0];
        let paid = raw(&row["spent"]);
        let received = raw(&row["received"]);
        let (fill, next) = s.trade(zfo, true, paid).unwrap();
        assert_eq!((fill.amount_in, fill.amount_out), (paid, received));
        assert_eq!(
            next.reserves,
            [raw(&row["post_reserves"][0]), raw(&row["post_reserves"][1])]
        );
        let (inverse, _) = s.trade(zfo, false, received).unwrap();
        assert!(inverse.amount_in <= paid && inverse.amount_out >= received);
        assert!(
            s.trade(zfo, true, inverse.amount_in - U256::ONE)
                .is_none_or(|(v, _)| v.amount_out < received)
        );
        assert!(row["strict"].is_object() && row["expired"].is_object());
    }
}
