//! Файл: tests/bsc_dex_maverick_math.rs
//! Что делает: базовая математика Maverick V2 (math/maverick_math.rs + math/maverick_tick_math.rs, решения 80, 73) на
//! ОФИЦИАЛЬНЫХ числовых векторах контракта maverickprotocol/v2-common (HEAD d91e8714a20b65913166d6b6608590ff822c3f06).
//! Без сети, без моков — только зафиксированные официальные значения из тестов контракта.
//!
//! Откуда векторы (решение 23):
//! - Math.sol → test/Math.t.sol: testInv / testClip / testMulDiv / testScale / testAbs32 / testAbs / testFloor / testMin
//!   / testMax — точные (бит-в-бит) равенства; testSqrt — свойство floor-корня (z²≤x<(z+1)²), повторено на
//!   фиксированных входах, плюс точные совершенные квадраты.
//! - TickMath.sol → test/TickMath.t.sol: testSqrtPrice — 6 жёстко заданных кейсов tickSqrtPrice с допуском
//!   assertApproxEqRel 0.000001e18 (1e-6 относительно), воспроизведён тем же допуском; testGetTickLFixed —
//!   getTickL(1, 0, sqrtLower, sqrtUpper) == 0.
//! - getSqrtPrice / getTickSqrtPriceAndL проверяются свойствами официального testGetSqrtPriceAndL (sqrtLower ≤ price ≤
//!   sqrtUpper; компоненты getTickSqrtPriceAndL совпадают с getTickL/getSqrtPrice) на тех же фиксированных входах —
//!   точного числового вектора у этих функций в официальных тестах нет (только fuzz/инвариант).
//!
//! НЕ покрыто (нет в исходнике v2-common; НЕ гадаем): сам цикл свопа Maverick V2 (проход по бинам) — его в v2-common
//! нет, он в контракте пула v2-core; портируется отдельной задачей. PoolLib.sol здесь — математика добавления
//! ликвидности (binReserves / reserveValue / deltaTickBalanceFromDeltaLpBalance), в офиц. тестах только fuzz без
//! числовых векторов — в этой фазе не портируется.
//! Кто вызывает: cargo test. От чего зависит: arb::venues::bsc_dex::math::{maverick_math, maverick_tick_math}.

use alloy_primitives::{I256, U256};

use arb::venues::bsc_dex::math::maverick_math as m;
use arb::venues::bsc_dex::math::maverick_tick_math as tm;

/// 10^18.
fn one() -> U256 {
    m::one()
}
/// n·10^18.
fn e18(n: u64) -> U256 {
    U256::from(n) * one()
}
/// Разобрать десятичную строку в U256.
fn w(s: &str) -> U256 {
    s.parse().unwrap()
}

// ======================= Math.sol (test/Math.t.sol) =======================

/// testInv: invFloor/invCeil точные равенства.
#[test]
fn math_inv() {
    assert_eq!(m::inv_floor(e18(3)).unwrap(), w("333333333333333333"));
    assert_eq!(m::inv_floor(e18(1)).unwrap(), e18(1));
    assert_eq!(m::inv_floor(e18(10)).unwrap(), w("100000000000000000"));
    assert_eq!(m::inv_ceil(e18(3)).unwrap(), w("333333333333333334"));
}

/// testClip.
#[test]
fn math_clip() {
    assert_eq!(m::clip(U256::from(10u8), U256::from(5u8)), U256::from(5u8));
    assert_eq!(m::clip(U256::from(5u8), U256::from(5u8)), U256::ZERO);
    assert_eq!(m::clip(U256::from(1u8), U256::from(5u8)), U256::ZERO);
    assert_eq!(m::clip128(10, 5), 5);
    assert_eq!(m::clip128(5, 5), 0);
    assert_eq!(m::clip128(1, 5), 0);
}

/// testMulDiv: mulDiv/mulFloor/mulCeil/divFloor/divCeil.
#[test]
fn math_muldiv() {
    let (ten, two, three) = (U256::from(10u8), U256::from(2u8), U256::from(3u8));
    assert_eq!(m::mul_div_ceil(ten, ten, two).unwrap(), U256::from(50u8));
    assert_eq!(m::mul_div_ceil(ten, ten, three).unwrap(), U256::from(34u8));

    // mulFloor(1e18+1, 333) + 1 == mulCeil(1e18+1, 333)
    let a = one() + U256::from(1u8);
    let f = m::mul_floor(a, U256::from(333u16)).unwrap();
    let c = m::mul_ceil(a, U256::from(333u16)).unwrap();
    assert_eq!(f + U256::from(1u8), c);
    assert_eq!(
        m::mul_floor(one(), one()).unwrap(),
        m::mul_ceil(one(), one()).unwrap()
    );

    // divFloor(1e18+1, 333e18) + 1 == divCeil(1e18+1, 333e18)
    let d333 = e18(333);
    let df = m::div_floor(a, d333).unwrap();
    let dc = m::div_ceil(a, d333).unwrap();
    assert_eq!(df + U256::from(1u8), dc);
    assert_eq!(
        m::div_floor(one(), one()).unwrap(),
        m::div_ceil(one(), one()).unwrap()
    );
}

/// testScale: scale / tokenScaleToAmmScale / ammScaleToTokenScale.
#[test]
fn math_scale() {
    let s12 = m::scale(12);
    let s18 = m::scale(18);
    assert_eq!(s12, w("1000000"));
    assert_eq!(s18, U256::from(1u8));

    assert_eq!(m::token_scale_to_amm_scale(w("1000"), s12), w("1000000000"));
    assert_eq!(m::token_scale_to_amm_scale(w("1000000000000"), s12), one());
    assert_eq!(m::token_scale_to_amm_scale(w("1000"), s18), w("1000"));

    assert_eq!(
        m::amm_scale_to_token_scale(w("1000"), s12, false),
        U256::ZERO
    );
    assert_eq!(
        m::amm_scale_to_token_scale(w("1000000000000"), s12, false),
        w("1000000")
    );
    assert_eq!(
        m::amm_scale_to_token_scale(w("1000000000000"), s18, false),
        w("1000000000000")
    );
    assert_eq!(
        m::amm_scale_to_token_scale(w("1000"), s12, true),
        U256::from(1u8)
    );
    assert_eq!(
        m::amm_scale_to_token_scale(w("1000000000000"), s12, true),
        w("1000000")
    );
    assert_eq!(
        m::amm_scale_to_token_scale(w("1001"), s12, true),
        U256::from(1u8)
    );
    assert_eq!(
        m::amm_scale_to_token_scale(w("1000000000001"), s12, true),
        w("1000001")
    );
}

/// testAbs32 / testAbs.
#[test]
fn math_abs() {
    assert_eq!(m::abs32(-10), 10);
    assert_eq!(m::abs32(10), 10);
    assert_eq!(m::abs32(i32::MIN), 2_147_483_648);

    assert_eq!(m::abs(I256::try_from(-10i64).unwrap()), U256::from(10u8));
    assert_eq!(m::abs(I256::try_from(10i64).unwrap()), U256::from(10u8));
    assert_eq!(m::abs(I256::MIN), U256::from(1u8) << 255);
    assert_eq!(
        m::abs(I256::MAX),
        (U256::from(1u8) << 255) - U256::from(1u8)
    );
}

/// testFloor: floorD8Unchecked(-10.01e8) = -11; int32::MIN·1e8 с дробью → откат (None).
#[test]
fn math_floor_d8() {
    assert_eq!(
        m::floor_d8_unchecked(I256::try_from(-1_001_000_000i64).unwrap()),
        Some(-11)
    );
    assert_eq!(
        m::floor_d8_unchecked(I256::try_from(-214_748_364_801_000_000i64).unwrap()),
        None
    );
}

/// testMin / testMax — выборка точных равенств (uint и int).
#[test]
fn math_min_max() {
    assert_eq!(m::min(U256::from(10u8), U256::from(1u8)), U256::from(1u8));
    assert_eq!(
        m::max(U256::from(10u8), U256::from(1100u16)),
        U256::from(1100u16)
    );
    assert_eq!(m::min128(10, 1100), 10);
    assert_eq!(m::max128(0, 1100), 1100);
    assert_eq!(
        m::min_i(
            I256::try_from(-10i64).unwrap(),
            I256::try_from(1i64).unwrap()
        ),
        I256::try_from(-10i64).unwrap()
    );
    assert_eq!(
        m::max_i(
            I256::try_from(-10i64).unwrap(),
            I256::try_from(-1i64).unwrap()
        ),
        I256::try_from(-1i64).unwrap()
    );
}

/// testSqrt: свойство floor-корня z²≤x<(z+1)² на фиксированных входах + точные совершенные квадраты.
#[test]
fn math_sqrt() {
    for x in [
        U256::ZERO,
        U256::from(1u8),
        U256::from(2u8),
        U256::from(3u8),
        U256::from(15u8),
        U256::from(16u8),
        e18(1),
        e18(2),
        w("1000000000000000000000000000000000000"), // 1e36
        U256::MAX,
    ] {
        let z = m::sqrt(x);
        assert!(z * z <= x, "z² ≤ x при x={x}");
        let up = z + U256::from(1u8);
        if up < (U256::from(1u8) << 128) {
            assert!(up * up > x, "(z+1)² > x при x={x}");
        }
    }
    // Точные совершенные квадраты.
    assert_eq!(m::sqrt(U256::from(16u8)), U256::from(4u8));
    assert_eq!(m::sqrt(e18(1)), w("1000000000")); // sqrt(1e18) = 1e9
    assert_eq!(m::sqrt(w("1000000000000000000000000000000000000")), e18(1)); // sqrt(1e36) = 1e18
}

// ===================== TickMath.sol (test/TickMath.t.sol) =====================

/// Относительное сравнение как assertApproxEqRel контракта: |a−b|/expected ≤ tol/1e18.
fn approx_rel(actual: U256, expected: U256, tol_e18: u64) {
    let diff = if actual > expected {
        actual - expected
    } else {
        expected - actual
    };
    let lhs = diff * one();
    let rhs = expected * U256::from(tol_e18);
    assert!(
        lhs <= rhs,
        "rel diff слишком велик: actual={actual}, expected={expected}"
    );
}

/// testSqrtPrice: 6 официальных кейсов tickSqrtPrice (допуск 0.000001e18 = 1e-6).
#[test]
fn tick_sqrt_price_official() {
    let tol = 1_000_000_000_000u64; // 0.000001e18
    let cases: [(i32, u64, &str); 6] = [
        (-2235, 1, "894272792908135808"),
        (0, 1, "1000000000000000000"),
        (1341, 1, "1069345359537797120"),
        (123, 2, "1012375333531026560"),
        (13, 12314, "2992007427897804980224"),
        (-5, 54269, "1282663628176"),
    ];
    for (tick, spacing, expected) in cases {
        let got = tm::tick_sqrt_price(U256::from(spacing), tick).unwrap();
        approx_rel(got, w(expected), tol);
    }
}

/// testGetTickLFixed: getTickL(1, 0, sqrtLower, sqrtUpper) == 0 (tick=0, tickSpacing=10000, sqrtUpper=sqrtLower·2.7001).
#[test]
fn get_tick_l_fixed() {
    let sqrt_lower = tm::tick_sqrt_price(U256::from(10_000u32), 0).unwrap();
    let sqrt_upper = m::mul_div_floor(sqrt_lower, w("2700100000000000000"), one()).unwrap(); // sqrtLower·2.7001e18/1e18
    let l = tm::get_tick_l(U256::from(1u8), U256::ZERO, sqrt_lower, sqrt_upper).unwrap();
    assert_eq!(l, U256::ZERO);
}

/// getSqrtPrice / getTickSqrtPriceAndL — свойства официального testGetSqrtPriceAndL на фиксированных входах:
/// sqrtLower ≤ price ≤ sqrtUpper, и компоненты getTickSqrtPriceAndL совпадают с getTickL/getSqrtPrice.
#[test]
fn get_sqrt_price_and_l_consistency() {
    let tick_spacing = U256::from(10u32);
    for (a, b, tick) in [
        (e18(5), e18(7), 100i32),
        (e18(1), e18(1), 0),
        (e18(9), U256::from(1u8), -50),
        (U256::from(1u8), e18(3), 200),
    ] {
        let (lo, hi) = tm::tick_sqrt_prices(tick_spacing, tick).unwrap();
        let l = tm::get_tick_l(a, b, lo, hi).unwrap();
        let p = tm::get_sqrt_price(a, b, lo, hi, l).unwrap();
        assert!(
            p >= lo && p <= hi,
            "цена внутри тика: lo={lo}, p={p}, hi={hi}"
        );
        let (p2, l2) = tm::get_tick_sqrt_price_and_l(a, b, lo, hi).unwrap();
        assert_eq!(l2, l);
        assert_eq!(p2, p);
    }
}
