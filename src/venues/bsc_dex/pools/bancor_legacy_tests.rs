//! Пять адресов из оригинального исполнения и обе физические площадки в ключе поиска.
use super::{
    bancor_legacy::Roles,
    bancor_legacy_bridge::{Neighbours, parse, paths},
};
use crate::core::types::{
    PoolRef,
    bancor_legacy::{BancorLegacyBridge, BancorLegacyState},
};
use alloy_primitives::{Address, U256};
use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::HashMap;
#[test]
fn discovers_both_converters_of_every_paid_five_address_path() {
    fn fixture(bytes: &[u8]) -> Value {
        serde_json::from_reader(GzDecoder::new(bytes)).unwrap()
    }
    fn addr(x: &Value) -> Address {
        x.as_str().unwrap().parse().unwrap()
    }
    let registry = fixture(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/bancor_v21_ethereum.json.gz"
    ));
    let payments = fixture(include_bytes!(
        "../../../../tests/data/first_run_dex_2026-11-26/bancor_v21_wallet.json.gz"
    ));
    let roles = Roles {
        registry: addr(&registry["roles"]["BancorConverterRegistry"]["address"]),
        network: addr(&registry["roles"]["BancorNetwork"]["address"]),
        native: Address::repeat_byte(0xee),
        wrapped: "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
            .parse()
            .unwrap(),
    };
    let mut graph = Neighbours::new();
    let mut members = HashMap::new();
    for m in registry["markets"].as_array().unwrap() {
        let pool = addr(&m["converter"]);
        members.insert(addr(&m["anchor"]), pool);
        let a = addr(&m["reserves"][0]);
        let b = addr(&m["reserves"][1]);
        for (x, y) in [(a, b), (b, a)] {
            graph
                .entry(roles.normalize(x))
                .or_default()
                .entry(y)
                .or_default()
                .insert(pool);
        }
    }
    let mut seen = 0;
    for tx in payments["controls"].as_array().unwrap() {
        if tx["mode"] != "wallet" {
            continue;
        }
        let actual: Vec<_> = tx["path"].as_array().unwrap().iter().map(addr).collect();
        if actual.len() != 5 {
            continue;
        }
        let input = roles.normalize(actual[0]);
        let output = roles.normalize(actual[4]);
        let mut forward = actual.clone();
        if input > output {
            forward.reverse()
        }
        let p = members[&forward[1]];
        let q = members[&forward[3]];
        let expected = PoolRef::Singleton {
            manager: p,
            id: q.into_word(),
        };
        assert!(
            paths(&graph, input, output, &roles)
                .iter()
                .any(|s| parse(s) == Some(expected.clone()))
        );
        let s = BancorLegacyState {
            converter: p,
            anchor: forward[1],
            second: Some(BancorLegacyBridge {
                converter: q,
                anchor: forward[3],
                middle: forward[2],
                fee_ppm: 0,
            }),
            network: roles.network,
            tokens: [roles.normalize(forward[0]), roles.normalize(forward[4])],
            reserves: [forward[0], forward[4]],
            balances: [U256::ZERO; 2],
            wallet_balances: [U256::ZERO; 2],
            fee_ppm: 0,
            levels: Default::default(),
        };
        assert_eq!(s.path(input).unwrap(), actual);
        assert_eq!(
            s.path(output).unwrap(),
            actual.into_iter().rev().collect::<Vec<_>>()
        );
        seen += 1;
    }
    assert_eq!(seen, 8);
}
