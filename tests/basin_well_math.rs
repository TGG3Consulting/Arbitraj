//! Файл: tests/basin_well_math.rs
//! Что проверяет: решение 485 — функции скважины Basin (math/basin_well.rs) на ОФИЦИАЛЬНЫХ векторах тестов репозитория
//! BeanstalkFarms/Basin@ecf69233 (последняя правка файлов тестов — d939f82): test/functions/Stable2.t.sol (Stable2 с
//! таблицей Stable2LUT1 — getAParameter() = 100, L19-L21 таблицы) и test/functions/ConstantProduct2.t.sol. Без записи
//! узла: векторы — из официального репозитория (правило агентов: чистые тесты математики на официальных векторах).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::math::basin_well, arb::core::types (BasinWellData).

use alloy_primitives::{Address, U256};

use arb::core::types::{BasinFunction, BasinWellData};
use arb::venues::bsc_dex::math::basin_well::{
    cp2_reserve, cp2_supply, stable2_reserve, stable2_supply, swap_out,
};

/// Число из десятичной строки (векторы тестов Basin записаны десятичными литералами).
fn u(s: &str) -> U256 {
    U256::from_str_radix(s, 10).expect("число")
}

/// A таблицы Stable2LUT1 (getAParameter() = 100: A = 1 при A_PRECISION = 100).
fn a() -> U256 {
    U256::from(100u8)
}

/// Stable2.t.sol L16-L38, L60-L120: calcLpTokenSupply и calcReserve в состояниях A, B, C.
#[test]
fn stable2_official_vectors() {
    let e18 = u("10000000000000000000");
    let (a_lp, b_lp, c_lp) = (
        u("20000000000000000000"),
        u("29405570361996060057"),
        u("44906735116816626495"),
    );
    let b_b1 = u("20000000");
    // calcLpTokenSupply: A (18, 18) и B (18, 6).
    assert_eq!(stable2_supply([e18, e18], a(), [18, 18]), Some(a_lp));
    assert_eq!(stable2_supply([e18, b_b1], a(), [18, 6]), Some(b_lp));
    // calcReserve: A — обе позиции.
    assert_eq!(
        stable2_reserve([U256::ZERO, e18], 0, a_lp, a(), [18, 18]),
        Some(e18)
    );
    assert_eq!(
        stable2_reserve([e18, U256::ZERO], 1, a_lp, a(), [18, 18]),
        Some(e18)
    );
    // C (12, 18): reserves[0] = 20e12, найти reserves[1] = 25e18.
    let c_b0 = u("20000000000000");
    let c_b1 = u("25000000000000000000");
    assert_eq!(
        stable2_reserve([c_b0, U256::ZERO], 1, c_lp, a(), [12, 18]),
        Some(c_b1)
    );
    // B (18, 6): обе позиции.
    assert_eq!(
        stable2_reserve([U256::ZERO, b_b1], 0, b_lp, a(), [18, 6]),
        Some(e18)
    );
    assert_eq!(
        stable2_reserve([e18, U256::ZERO], 1, b_lp, a(), [18, 6]),
        Some(b_b1)
    );
    // Знаков больше 18 — откат decodeWellData (InvalidTokenDecimals).
    assert_eq!(stable2_supply([e18, e18], a(), [19, 18]), None);
}

/// ConstantProduct2.t.sol L11-L23, L48-L121: calcLpTokenSupply и calcReserve (EXP_PRECISION = 1e12).
#[test]
fn cp2_official_vectors() {
    let e18 = u("10000000000000000000");
    let a_lp = u("10000000000000000000000000");
    assert_eq!(cp2_supply(e18, e18), Some(a_lp));
    let (b0, b1) = (u("1000000000000000000"), u("1250000000"));
    let b_lp = u("35355339059327376220");
    assert_eq!(cp2_supply(b0, b1), Some(b_lp));
    assert_eq!(cp2_reserve(e18, a_lp), Some(e18));
    assert_eq!(cp2_reserve(b1, b_lp), Some(b0));
    assert_eq!(cp2_reserve(b0, b_lp), Some(b1));
    let (c0, c1, c_lp) = (
        u("20000000000000000000"),
        u("31250000000000000000"),
        u("25000000000000000000000000"),
    );
    assert_eq!(cp2_reserve(c0, c_lp), Some(c1));
}

/// Обмен скважины (Well.sol L247-L253) на состоянии A ConstantProduct2: вход прибавляется к резерву i, выход =
/// резерв_j − calcReserve; токен не из скважины и резерв больше uint128 — None (откат InvalidTokens / storeUint128).
#[test]
fn swap_out_shape() {
    let e18 = u("10000000000000000000");
    let (t0, t1) = (Address::repeat_byte(0x22), Address::repeat_byte(0x11));
    let d = BasinWellData {
        token0: t0,
        token1: t1,
        reserve0: e18,
        reserve1: e18,
        supply: u("10000000000000000000000000"),
        function: BasinFunction::ConstantProduct2,
    };
    let amount = u("1000000000000000000");
    // 10e18 − ceil(1e50 / (11e18·1e12)) = 10e18 − 9090909090909090910 = 909090909090909090.
    let want = e18 - u("9090909090909090910");
    assert_eq!(swap_out(&d, t0, amount), Some(want));
    assert_eq!(swap_out(&d, t1, amount), Some(want));
    assert_eq!(swap_out(&d, Address::ZERO, amount), None);
    assert_eq!(swap_out(&d, t0, U256::from(u128::MAX)), None);
}
