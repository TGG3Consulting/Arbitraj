//! Файл: tests/mid_dex_common/rec.rs
//! Что делает: чтение записи с цепи tests/data/fstswap_fist_2026-11-14/calls.json (eth_call, один блок) для тестов
//! решения 276: адреса токенов, symbol() по ABI, резервы пар fstswap.
//! Кто вызывает: tests/bsc_dex_mid_dex.rs, tests/bsc_dex_mid_dex_pick.rs, tests/bsc_dex_mid_dex_read.rs.
//! От чего зависит: arb::venues::bsc_dex::node::{abi, call}, serde_json.

use alloy_primitives::{Address, U256};
use serde_json::Value;

use arb::venues::bsc_dex::node::abi::IERC20;
use arb::venues::bsc_dex::node::call::decode;

/// Запись целиком.
pub fn rec() -> Value {
    serde_json::from_str(include_str!("../data/fstswap_fist_2026-11-14/calls.json")).unwrap()
}

/// Байты из `0x…`.
pub fn hex(s: &str) -> Vec<u8> {
    alloy_primitives::hex::decode(s.trim_start_matches("0x")).unwrap()
}

/// Адрес токена записи по ключу («FIST», «WRAPPER», «USDT», «ETH»).
pub fn addr(v: &Value, k: &str) -> Address {
    v["tokens"][k].as_str().unwrap().parse().unwrap()
}

/// Ответ под ключом k, разобранный как symbol() (None — не по ABI string).
pub fn symbol(v: &Value, k: &str) -> Option<String> {
    decode::<IERC20::symbolCall>(&hex(v[k].as_str().unwrap())).ok()
}

/// Резервы (r0, r1) из ответа getReserves() записи.
pub fn reserves(v: &Value, pair: &str) -> (u128, u128) {
    let raw = hex(v["pairs"][pair]["getReserves()_0x0902f1ac"]
        .as_str()
        .unwrap());
    let w = |i: usize| U256::from_be_slice(&raw[i * 32..i * 32 + 32]).to::<u128>();
    (w(0), w(1))
}
