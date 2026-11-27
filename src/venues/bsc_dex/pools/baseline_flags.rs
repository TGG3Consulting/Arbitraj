//! Связывает биты разрешений физического V4-хука с проверенными маршрутами Relay.
use alloy_primitives::{Address, FixedBytes, keccak256};
use std::collections::BTreeSet;

const CALLBACKS: [&str; 10] = [
    "beforeInitialize(address,(address,address,uint24,int24,address),uint160)",
    "afterInitialize(address,(address,address,uint24,int24,address),uint160,int24)",
    "beforeAddLiquidity(address,(address,address,uint24,int24,address),(int24,int24,int256,bytes32),bytes)",
    "afterAddLiquidity(address,(address,address,uint24,int24,address),(int24,int24,int256,bytes32),int256,int256,bytes)",
    "beforeRemoveLiquidity(address,(address,address,uint24,int24,address),(int24,int24,int256,bytes32),bytes)",
    "afterRemoveLiquidity(address,(address,address,uint24,int24,address),(int24,int24,int256,bytes32),int256,int256,bytes)",
    "beforeSwap(address,(address,address,uint24,int24,address),(bool,int256,uint160),bytes)",
    "afterSwap(address,(address,address,uint24,int24,address),(bool,int256,uint160),int256,bytes)",
    "beforeDonate(address,(address,address,uint24,int24,address),uint256,uint256,bytes)",
    "afterDonate(address,(address,address,uint24,int24,address),uint256,uint256,bytes)",
];

pub(super) fn covered(relay: Address, routes: &BTreeSet<FixedBytes<4>>) -> bool {
    let flags = u16::from_be_bytes([relay[18], relay[19]]);
    let enabled = |bit: usize| flags & (1u16 << bit) != 0;
    // Нижние четыре бита меняют форму дельты соответствующего callback.
    let deltas = [(3, 7), (2, 6), (1, 10), (0, 8)];
    deltas.iter().all(|&(a, b)| !enabled(a) || enabled(b))
        && CALLBACKS.iter().enumerate().all(|(i, signature)| {
            !enabled(13 - i) || routes.contains(&FixedBytes::from_slice(&keccak256(signature)[..4]))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn every_enabled_callback_in_the_archive_must_have_a_verified_route() {
        let raw = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/first_run_dex_2026-11-26/baseline_base_components.json.gz"
        ));
        let f: Value = serde_json::from_reader(flate2::read::GzDecoder::new(&raw[..])).unwrap();
        let relay: Address = f["relay"].as_str().unwrap().parse().unwrap();
        let routes: BTreeSet<_> = f["routes"]
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.parse::<FixedBytes<4>>().unwrap())
            .collect();
        assert!(covered(relay, &routes));
        let mut tested = 0;
        for sig in CALLBACKS {
            let id = FixedBytes::from_slice(&keccak256(sig)[..4]);
            if f["routes"][id.to_string()] == f["components"]["BHook"] {
                let mut missing = routes.clone();
                missing.remove(&id);
                assert!(!covered(relay, &missing));
                tested += 1;
            }
        }
        assert_eq!(tested, 5);
    }
}
