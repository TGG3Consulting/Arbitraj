//! Формат readSlot и ценовых границ по ответам mainnet и оплаченным userCmd.
use super::{FLAG_CROSS_CALLPATH, SWAP_CALLPATH, proxy_slot};
use crate::core::types::ambient::Market;
use crate::venues::bsc_dex::pools::ambient::{ICroc, source};
use alloy::sol_types::{SolCall, SolValue};
use alloy_primitives::{Address, Bytes, U256, keccak256};
use flate2::read::GzDecoder;
use serde_json::Value;

fn payments() -> Value {
    serde_json::from_reader(GzDecoder::new(
        &include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/ambient_ethereum_payments.json.gz")[..],
    )).unwrap()
}

#[test]
fn hot_and_flag_cross_resolve_to_the_recorded_implementations() {
    let archive: Value = serde_json::from_str(include_str!(
        "../../../../tests/data/first_run_dex_2026-11-26/ambient_ethereum_proxy_slots.json"
    )).unwrap();
    let evidence = payments();
    for (i, path) in [SWAP_CALLPATH, FLAG_CROSS_CALLPATH].into_iter().enumerate() {
        let params = &archive["queries"][i]["params"];
        assert_eq!(params[1], evidence["head"]);
        assert_eq!(params[0]["to"], evidence["roles"]["dex"]["address"]);
        let raw: Bytes = params[0]["data"].as_str().unwrap().parse().unwrap();
        assert_eq!(ICroc::readSlotCall { slot: proxy_slot(path) }.abi_encode(), raw.to_vec());
        let reply: Bytes = archive["replies"][i]["result"].as_str().unwrap().parse().unwrap();
        let found = ICroc::readSlotCall::abi_decode_returns(&reply).unwrap();
        let role: Address = evidence["roles"][path.to_string()]["address"].as_str().unwrap().parse().unwrap();
        assert_eq!(found, U256::from_be_slice(role.as_slice()));
    }
}

#[test]
fn impact_price_limits_match_the_original_paid_command_in_both_directions() {
    let evidence = payments();
    let impact: Address = evidence["roles"]["impact"]["address"].as_str().unwrap().parse().unwrap();
    let mut directions = [false; 2];
    for leg in evidence["payments"].as_array().unwrap() {
        let input: Address = leg["source"].as_str().unwrap().parse().unwrap();
        let output: Address = leg["target"].as_str().unwrap().parse().unwrap();
        let original: Bytes = leg["original"]["data"].as_str().unwrap().parse().unwrap();
        assert_eq!(&original[..4], &keccak256("userCmd(uint16,bytes)")[..4]);
        let (path, packet) = <(u16, Bytes)>::abi_decode_params(&original[4..]).unwrap();
        assert_eq!(path, SWAP_CALLPATH);
        let market = Market {
            base: input.min(output),
            quote: input.max(output),
            index: U256::from_be_slice(&packet[64..96]),
        };
        let buy = input == market.base;
        let qty: U256 = leg["requested"].as_str().unwrap().parse().unwrap();
        let fixed_input = leg["exact_input"].as_bool().unwrap();
        let (_, call) = source(impact, market, buy, fixed_input, qty).unwrap();
        // Первые восемь полей команды HotPath совпадают с аргументами calcImpact.
        assert_eq!(&call[4..], &packet[..8 * 32]);
        directions[usize::from(buy)] = true;
    }
    assert_eq!(directions, [true; 2]);
}
