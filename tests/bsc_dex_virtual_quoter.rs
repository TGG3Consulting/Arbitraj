//! Файл: tests/bsc_dex_virtual_quoter.rs
//! Что проверяет: решение 186 — виртуальный квотер: код из contracts/out проходит сверку хеша; адрес подмены
//! постоянный; ответ (used, out) разбирается — взял весь вход → выход, меньше → «не хватает ликвидности»; виды пулов.
//! Живая сверка с официальным QuoterV2 на узле — промпт серверу (docs/TASKS.md, решение 186).
//! Кто вызывает: cargo test.

use alloy_primitives::U256;
use arb::venues::bsc_dex::pools::families::Proto;
use arb::venues::bsc_dex::quote::virtual_quoter as vq;

#[test]
fn runtime_hash_matches_and_address_is_stable() {
    let code = vq::runtime().unwrap();
    assert!(code.len() > 100);
    assert_eq!(vq::address(), vq::address());
}

#[test]
fn decode_full_and_short() {
    let enc = |used: u64, out: u64| {
        let mut b = vec![0u8; 64];
        b[24..32].copy_from_slice(&used.to_be_bytes());
        b[56..64].copy_from_slice(&out.to_be_bytes());
        b
    };
    let amount = U256::from(1000u64);
    assert_eq!(
        vq::decode(&enc(1000, 997), amount),
        Some(Ok(U256::from(997u64)))
    );
    assert_eq!(vq::decode(&enc(400, 390), amount), Some(Err(vq::SHORT)));
    assert_eq!(vq::decode(&[1, 2, 3], amount), None);
}

#[test]
fn supported_kinds() {
    assert!(vq::supports(Proto::UniswapV3));
    assert!(vq::supports(Proto::PancakeV3));
    assert!(vq::supports(Proto::Algebra));
    assert!(vq::supports(Proto::Slipstream));
    assert!(!vq::supports(Proto::V2));
}
