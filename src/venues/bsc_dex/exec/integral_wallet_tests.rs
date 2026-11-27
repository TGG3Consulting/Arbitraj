//! Квитанции Integral SIZE до закрытия: подписанные байты, кошелёк, отдельный заказ Delay и восстановление.
use super::*;
use crate::venues::bsc_dex::check::simulate::SimLog;
use serde_json::Value;
const D: &[u8] = include_bytes!(
    "../../../../tests/data/first_run_dex_2026-11-26/integral_size_ethereum.json.gz"
);
const N: &[u8] = include_bytes!(
    "../../../../tests/data/first_run_dex_2026-11-26/integral_size_ethereum_wallets.json.gz"
);
const M: &[u8] = include_bytes!(
    "../../../../tests/data/first_run_dex_2026-11-26/integral_size_ethereum_protected.json.gz"
);
fn read(raw: &[u8]) -> Value {
    serde_json::from_reader(flate2::read::GzDecoder::new(raw)).unwrap()
}
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn amount(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
#[test]
fn restored_orders_use_original_wallet_destination_and_single_instant_fill() {
    let d = read(D);
    let roles = &d["roles"];
    let code = Code {
        shell: roles["relayerProxy"]["keccak256"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
        implementation: addr(&roles["relayer"]["address"]),
        runtime: roles["relayer"]["keccak256"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
    };
    let historical = read(N);
    let protected = read(M);
    let relay = addr(&roles["relayerProxy"]["address"]);
    let delay = addr(&roles["delay"]["address"]);
    let pair = addr(&protected["cases"][0]["pair"]);
    let mut count = [0; 2];
    for (module, cases, index) in [
        (false, &historical["historical"]["cases"], 9),
        (true, &protected["cases"], 11),
    ] {
        for row in cases
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| x["mode"] != "v5")
        {
            let call = &row["calls"][index];
            let target = addr(&call["to"]);
            let payer = addr(&row["payer"]);
            let calldata: Bytes = call["data"].as_str().unwrap().parse().unwrap();
            let w = |i: usize| U256::from_be_slice(&calldata[4 + i * 32..36 + i * 32]);
            let (input, output) = (addr(&row["input"]), addr(&row["output"]));
            let buy = row["mode"] == "module-buy" || row["side"] == "buy";
            let p = Proof {
                terms: Terms {
                    pair,
                    target,
                    relay,
                    delay,
                    code,
                    desired: buy.then_some(w(3)),
                },
                payer,
                input,
                output,
                amount: w(2),
                minimum: w(3),
                deadline: w(if module { 5 } else { 6 }),
            };
            assert_eq!(p.data(), calldata);
            let p: Proof = serde_json::from_value(serde_json::to_value(p).unwrap()).unwrap();
            let logs: Vec<SimLog> =
                serde_json::from_value(row["results"][index]["logs"].clone()).unwrap();
            let paid = amount(&row[if module { "spent" } else { "paid" }]);
            let net = amount(&row[if module { "received" } else { "net" }]);
            assert_eq!(p.measured(&logs), Some((paid, net)));
            let actual = signed_call::logs(&logs).unwrap();
            assert!(p.amounts(&actual, Address::ZERO, input, output).is_none());
            let keep: Vec<_> = logs
                .iter()
                .filter(|l| l.address != relay)
                .cloned()
                .collect();
            assert!(p.measured(&keep).is_none());
            let mut duplicate = actual.clone();
            duplicate.push(
                actual
                    .iter()
                    .find(|l| {
                        l.address() == relay
                            && l.topic0().is_some_and(|t| {
                                *t == IRelay::Sell::SIGNATURE_HASH
                                    || *t == IRelay::Buy::SIGNATURE_HASH
                            })
                    })
                    .unwrap()
                    .clone(),
            );
            assert!(p.amounts(&duplicate, payer, input, output).is_none());
            count[usize::from(module)] += 1;
        }
    }
    assert_eq!(count, [8, 8]);
}
#[test]
fn disabled_pairs_remain_per_block_candidates() {
    use crate::venues::bsc_dex::pools::{families, per_block};
    for kind in ["integral_size", "integral_size_v5"] {
        let f = families::make("integral-size-ethereum", kind).unwrap();
        assert!(per_block::needs(f));
        assert!(f.tradable && f.graphable);
        assert_eq!(f.executor_kind.is_some(), kind == "integral_size_v5");
    }
}
