//! Файл: tests/bsc_dex_fstswap_fee.rs
//! Что делает: комиссия пар fstswap фактом (решение 276, ревью; docs/TASKS.md блок 276): на записи
//! tests/data/fstswap_fist_2026-11-14/fee_check.json (один блок 0x7f6a588) ответ роутера fstswap getAmountsOut на
//! паре FIST/USDT сравнивается с формулой V2 getAmountOut по getReserves той же пары того же блока для комиссий
//! 0,2 / 0,25 / 0,3 %. Совпадает до wei в обе стороны только 0,3 % (997/1000) — это значение для dex.v2_fee_pct
//! строки fstswap; 0,2 и 0,25 % — не совпадают.
//! Формула — UniswapV2Library.getAmountOut (v2-periphery ed249913, contracts/libraries/UniswapV2Library.sol):
//! out = in·(1−f)·rOut / (rIn + in·(1−f)) в целых числах.
//! Кто вызывает: cargo test.
//! От чего зависит: alloy_primitives, serde_json.

use alloy_primitives::U256;
use serde_json::Value;

fn word(v: &Value, k: &str, i: usize) -> U256 {
    let raw =
        alloy_primitives::hex::decode(v[k].as_str().unwrap().trim_start_matches("0x")).unwrap();
    U256::from_be_slice(&raw[i * 32..i * 32 + 32])
}

/// Формула V2 с комиссией num/den.
fn out(a: U256, r_in: U256, r_out: U256, num: u64, den: u64) -> U256 {
    let x = a * U256::from(den - num);
    x * r_out / (r_in * U256::from(den) + x)
}

#[test]
fn fstswap_fee_is_0_3_pct() {
    let v: Value =
        serde_json::from_str(include_str!("data/fstswap_fist_2026-11-14/fee_check.json")).unwrap();
    // token0 = USDT, token1 = FIST (факт записи).
    assert_eq!(
        word(&v, "pair.token0()_0x0dfe1681", 0),
        U256::from_str_radix("55d398326f99059ff775485246999027b3197955", 16).unwrap()
    );
    let (r_usdt, r_fist) = (
        word(&v, "pair.getReserves()_0x0902f1ac", 0),
        word(&v, "pair.getReserves()_0x0902f1ac", 1),
    );
    // getAmountsOut: [смещение, длина 2, amounts[0], amounts[1]].
    let k1 = "router.getAmountsOut(1000e6,[FIST,USDT])_0xd06ca61f";
    let k2 = "router.getAmountsOut(100e18,[USDT,FIST])_0xd06ca61f";
    let (in1, got1) = (word(&v, k1, 2), word(&v, k1, 3));
    let (in2, got2) = (word(&v, k2, 2), word(&v, k2, 3));
    let fit = |num: u64, den: u64| {
        out(in1, r_fist, r_usdt, num, den) == got1 && out(in2, r_usdt, r_fist, num, den) == got2
    };
    assert!(fit(3, 1000), "0,3 % совпадает до wei");
    assert!(!fit(2, 1000), "0,2 % не совпадает");
    assert!(!fit(25, 10000), "0,25 % не совпадает");
}
