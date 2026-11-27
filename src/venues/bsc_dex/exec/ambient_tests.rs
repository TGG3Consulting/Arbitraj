use super::*;
use flate2::read::GzDecoder;
use serde_json::Value;
fn asset(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn amount(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
#[test]
fn physical_keys_wallet_debits_and_exact_target_are_recoverable() {
    let data: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/ambient_ethereum_wallet.json.gz"
        )[..],
    ))
    .unwrap();
    let (owner, module) = (asset(&data["owner"]), asset(&data["module"]));
    let mut counts = [0; 2];
    for row in data["payments"].as_array().unwrap() {
        let a = &row["market"];
        let m = Market {
            base: asset(&a[0]),
            quote: asset(&a[1]),
            index: U256::from(a[2].as_u64().unwrap()),
        };
        assert_eq!(
            m.id(),
            row["pool_key"].as_str().unwrap().parse::<B256>().unwrap()
        );
        let input = asset(&row["input"]);
        let output = asset(&row["output"]);
        let spent = amount(&row["spent"]);
        let net = amount(&row["received"]);
        let logs: Vec<Log> = serde_json::from_value(row["logs"].clone()).unwrap();
        let paid: Vec<_> = logs
            .iter()
            .filter(|x| {
                x.address() == module && x.topic0() == Some(&abi::IWallet::Filled::SIGNATURE_HASH)
            })
            .map(|x| abi::IWallet::Filled::decode_log_data(&x.inner.data).unwrap())
            .collect();
        let [ev] = paid.as_slice() else {
            panic!("missing unique Filled")
        };
        assert_eq!(
            (ev.market, ev.input, ev.output, ev.spent, ev.received),
            (m.id(), input, output, spent, net)
        );
        let direct = row["mode"] == "direct";
        counts[usize::from(!direct)] += 1;
        if direct {
            let raw: Bytes = row["call"]["data"].as_str().unwrap().parse().unwrap();
            let wire = abi::IWallet::executeCall::abi_decode(&raw).unwrap();
            let proof = Proof {
                terms: Terms {
                    market: m,
                    target: module,
                    module_hash: data["manifest"]["runtimeKeccak"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap(),
                    desired: wire.exactOut.then(|| U256::from(wire.minimum)),
                },
                payer: owner,
                input,
                output,
                maximum: U256::from(wire.budget),
                minimum: U256::from(wire.minimum),
                deadline: wire.deadline,
            };
            assert_eq!(proof.data(), raw);
            assert_eq!(
                proof.amounts(&logs, owner, input, output),
                Some((spent, net))
            );
            assert!(proof.amounts(&logs, owner, output, input).is_none());
        } else {
            assert_eq!(ev.payer, asset(&row["call"]["to"]));
            assert_eq!(ev.recipient, owner);
        }
        assert!(row["strict"].is_object() && row["expired"].is_object());
    }
    assert_eq!(counts, [8, 4]);
    let limited = data["limited"].as_array().unwrap();
    assert!(limited.iter().any(|r| r["target"] == false
        && r["protected"] == "0x1"
        && amount(&r["original_spent"]) < amount(&r["requested"])));
    assert!(limited.iter().any(|r| r["target"] == true
        && r["protected"] == "0x0"
        && amount(&r["original_net"]) < amount(&r["requested"])));
}

#[test]
fn official_both_signed_flows_match_measured_orders_and_direction() {
    use crate::venues::bsc_dex::pools::ambient;
    use alloy::sol_types::SolValue;
    use std::collections::HashSet;
    let data: Value = serde_json::from_reader(GzDecoder::new(
        &include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/ambient_ethereum_payments.json.gz"
        )[..],
    ))
    .unwrap();
    let markets = data["states"].as_array().unwrap();
    let keys: HashSet<_> = markets
        .iter()
        .map(|r| {
            Market {
                base: asset(&r["base"]),
                quote: asset(&r["quote"]),
                index: U256::from(r["index"].as_u64().unwrap()),
            }
            .id()
        })
        .collect();
    assert_eq!(keys.len(), 127);
    let mut modes = [0; 4];
    let mut legs = HashSet::new();
    for p in data["ladder"].as_array().unwrap() {
        let buy = p["buy"].as_bool().unwrap();
        let fixed = p["fixed_base"].as_bool().unwrap();
        let base: i128 = p["base_flow"].as_str().unwrap().parse().unwrap();
        let quote: i128 = p["quote_flow"].as_str().unwrap().parse().unwrap();
        let price: u128 = p["end_price"].as_str().unwrap().parse().unwrap();
        let raw = (base, quote, price).abi_encode();
        let measured = ambient::decode(&raw, buy).unwrap();
        assert!(ambient::decode(&raw, !buy).is_none());
        let exact_in = buy == fixed;
        assert_eq!(
            if exact_in {
                measured.amount_in
            } else {
                measured.amount_out
            },
            amount(&p["requested"])
        );
        modes[2 * usize::from(buy) + usize::from(fixed)] += 1;
        let (base, quote) = (asset(&p["base"]), asset(&p["quote"]));
        legs.insert((
            if buy { base } else { quote },
            if buy { quote } else { base },
            exact_in,
            measured.amount_in,
            measured.amount_out,
        ));
    }
    assert!(modes.iter().all(|c| *c >= 2));
    let mut matched = 0;
    for payment in data["payments"].as_array().unwrap() {
        let (spent, net) = (amount(&payment["spent"]), amount(&payment["received"]));
        let exact_in = payment["exact_input"].as_bool().unwrap();
        assert!(!spent.is_zero() && !net.is_zero());
        assert_eq!(
            if exact_in { spent } else { net },
            amount(&payment["requested"])
        );
        let endpoints = (asset(&payment["source"]), asset(&payment["target"]));
        if legs.contains(&(endpoints.0, endpoints.1, exact_in, spent, net)) {
            matched += 1;
        }
    }
    // The wider ETH grid and the native small-size funded controls share six exact sizes.
    assert!(matched >= 6);
}
