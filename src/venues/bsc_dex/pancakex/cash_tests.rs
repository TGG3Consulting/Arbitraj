use super::Terms;
use super::cash::{Cash, NativePayment};
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, U256};
use serde_json::Value;

pub(super) fn paid(terms: &Terms, h: &Value) {
    let logs: Vec<Log> = serde_json::from_value(h["receipt"]["logs"].clone()).unwrap();
    let native: Vec<_> = h["native_recipient_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| NativePayment {
            from: v["from"].as_str().unwrap().parse().unwrap(),
            to: v["to"].as_str().unwrap().parse().unwrap(),
            value: v["amount"].as_str().unwrap().parse().unwrap(),
        })
        .collect();
    let multi = logs
        .iter()
        .any(|l| l.topics().first() == Some(&super::cash::FillData::SIGNATURE_HASH));
    let cash = terms.cash(&logs, &native, multi).unwrap();
    let expected = Cash {
        paid: h["paid"]["amount"].as_str().unwrap().parse().unwrap(),
        received: h["received"]["amount"].as_str().unwrap().parse().unwrap(),
        filler: h["order"]["filler"].as_str().unwrap().parse().unwrap(),
    };
    assert_eq!(cash, expected);
    let mut doubled = logs.clone();
    doubled.extend_from_slice(&logs);
    assert!(terms.cash(&doubled, &native, multi).is_none());
    if !native.is_empty() {
        assert!(terms.cash(&logs, &[], multi).is_none());
        let foreign = [NativePayment {
            from: Address::ZERO,
            to: terms.request.owner,
            value: U256::MAX,
        }];
        assert!(terms.cash(&logs, &foreign, multi).is_none());
    }
    if multi {
        let short: Vec<_> = logs
            .into_iter()
            .filter(|l| l.topics().first() != Some(&super::cash::FillData::SIGNATURE_HASH))
            .collect();
        assert!(terms.cash(&short, &native, true).is_none());
    }
}
