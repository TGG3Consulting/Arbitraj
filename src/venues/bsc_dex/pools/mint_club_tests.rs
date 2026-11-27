//! Check actual registration, rejected ERC1155 clones, read-only price limits and proof of exchange by the same Bond.
use super::*;
use crate::venues::bsc_dex::check::simulate::SimLog;
use alloy::sol_types::SolEvent;
use flate2::read::GzDecoder;
use serde_json::Value;
fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn num(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn bytes(v: &Value) -> Bytes {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn configured_market_is_a_registered_erc20_and_native_logs_distinguish_each_token() {
    let base = (
        include_str!("../../../../tests/data/first_run_dex_2026-11-26/mint_club_base_events.json"),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/mint_club_base_wallets.json.gz"
        )
        .as_slice(),
    );
    let eth = (
        include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/mint_club_ethereum_events.json"
        ),
        include_bytes!(
            "../../../../tests/data/first_run_dex_2026-11-26/mint_club_ethereum_wallets.json.gz"
        )
        .as_slice(),
    );
    for (raw, wallets) in [base, eth] {
        let primary: Value = serde_json::from_str(raw).unwrap();
        let wallets: Value = serde_json::from_reader(GzDecoder::new(wallets)).unwrap();
        assert_eq!(primary["selector"], wallets["selector"]);
        assert_eq!(primary["state_root"], wallets["state_root"]);
        let implementation = addr(&wallets["token_implementation"]);
        let bond = addr(&wallets["bond"]);
        let control = &primary["erc1155"];
        let nft = addr(&control["token"]);
        let nft_terms = bytes(&control["terms_raw"]);
        let nft_reserve = IBond::tokenBondCall::abi_decode_returns_validate(&nft_terms)
            .unwrap()
            .reserveToken;
        assert_eq!(listed(nft, nft_reserve, &nft_terms).unwrap(), [nft]);
        assert!(!clone_of(&bytes(&control["clone_code"]), implementation));
        for m in primary["markets"].as_array().unwrap() {
            let issued = addr(&m["token"]);
            let reserve = addr(&m["reserve"]);
            let log: alloy_primitives::Log = serde_json::from_value(m["creation"].clone()).unwrap();
            let event = IBond::TokenCreated::decode_log_data(&log.data).unwrap();
            assert_eq!(
                (log.address, event.token, event.reserveToken),
                (bond, issued, reserve)
            );
            assert_eq!(event.name, m["name"].as_str().unwrap());
            assert_eq!(event.symbol, m["symbol"].as_str().unwrap());
            assert!(clone_of(&bytes(&m["clone_code"]), implementation));
            let terms = bytes(&m["terms_raw"]);
            assert_eq!(listed(issued, reserve, &terms).unwrap(), [issued]);
            assert!(listed(issued, issued, &terms).unwrap().is_empty());
            let original = wallets["markets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["token"] == m["token"])
                .unwrap();
            let state = MintClubState {
                bond,
                token: issued,
                reserve,
                supply: num(&original["supply"]),
                steps: original["steps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        (
                            p[0].as_str().unwrap().parse().unwrap(),
                            p[1].as_str().unwrap().parse().unwrap(),
                        )
                    })
                    .collect(),
                mint_bps: original["mint_bps"].as_u64().unwrap() as u16,
                burn_bps: original["burn_bps"].as_u64().unwrap() as u16,
                bonded: num(&m["bonded"]),
                available: num(&m["available"]),
            };
            for raw in primary["historical_swaps"].as_array().unwrap() {
                let log: SimLog = serde_json::from_value(raw.clone()).unwrap();
                assert_eq!(emitted(&state, &log), log.topics[1] == issued.into_word());
            }
            for q in m["quotes"].as_array().unwrap() {
                let qty = num(&q["size"]);
                let fill = super::super::super::math::mint_club::fill(&state, reserve, false, qty)
                    .unwrap();
                assert_eq!(
                    answer(true, &bytes(&q["mint"])),
                    Some((fill.swapped.amount_in, fill.royalty))
                );
                if !q["burn_error"].is_null() {
                    assert!(qty > state.supply);
                    assert!(
                        super::super::super::math::mint_club::fill(&state, issued, true, qty)
                            .is_none()
                    );
                }
            }
            for trial in original["trials"].as_array().unwrap() {
                for kind in ["mint_logs", "burn_logs"] {
                    let logs: Vec<SimLog> = serde_json::from_value(trial[kind].clone()).unwrap();
                    assert_eq!(logs.iter().filter(|l| emitted(&state, l)).count(), 1);
                    let real = logs.iter().find(|l| emitted(&state, l)).unwrap();
                    let mut other = real.clone();
                    other.address = issued;
                    assert!(!emitted(&state, &other));
                    other = real.clone();
                    other.topics[1] = reserve.into_word();
                    assert!(!emitted(&state, &other));
                    other = real.clone();
                    other.topics[3] = issued.into_word();
                    assert!(!emitted(&state, &other));
                }
            }
        }
    }
}
