//! Файл: venues/bsc_dex/math/balancer_log_exp.rs
//! Что делает: порт LogExpMath Balancer V2 (решение 289) — x^y для фикс-точки 18 знаков через ln и exp рядами, как в
//! контракте пула: те же константы, те же шаги и то же целочисленное деление со знаком (Solidity: частное к нулю,
//! остаток со знаком делимого). Нужен весовой формуле (FixedPoint.powUp, math/balancer_math.rs).
//! Кто вызывает: math/balancer_math.rs.
//! От чего зависит: alloy-primitives (I256, U256).
//! Настройки: нет (константы — свойства протокола, ссылки на строки ниже).
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 v2/deprecated/20210418-weighted-pool/build-info/
//! WeightedPoolFactory.json → contracts/lib/math/LogExpMath.sol (тот же текст — у Sourcify пулов ACSI 0x7ea9f435,
//! 0x6e5dbc9a, exact_match; выдержка — tests/data/balancer_acsi_2026-11-15/source_excerpt.txt): константы L36-L86, pow
//! L93-L139, exp L146-L281, ln L286-L421, ln_36 L456-L503. Solidity 0.7 без проверок переполнения int256: в пределах
//! проверок pow (x < 2^255, y < MILD_EXPONENT_BOUND, произведение в [MIN, MAX]_NATURAL_EXPONENT) переполнений нет; здесь
//! переполнение — None (котировки нет), а не заворот.

use alloy_primitives::{I256, U256};

/// Целое со знаком из десятичной строки константы протокола. Строки ниже — числа исходника, разбор не падает.
fn c(s: &str) -> I256 {
    I256::from_dec_str(s).unwrap_or(I256::ZERO)
}

/// 10^n как I256. Ошибок нет (n ≤ 36).
fn ten(n: u32) -> I256 {
    I256::from_raw(U256::from(10u8).pow(U256::from(n)))
}

/// Деление как в Solidity (частное к нулю). None — деление на ноль.
fn sdiv(a: I256, b: I256) -> Option<I256> {
    if b.is_zero() {
        return None;
    }
    let q = I256::from_raw(a.unsigned_abs() / b.unsigned_abs());
    Some(if a.is_negative() != b.is_negative() {
        -q
    } else {
        q
    })
}

/// Остаток как в Solidity (знак делимого). None — деление на ноль.
fn smod(a: I256, b: I256) -> Option<I256> {
    let q = sdiv(a, b)?;
    a.checked_sub(q.checked_mul(b)?)
}

/// Умножение с проверкой переполнения (переполнение — None).
fn mul(a: I256, b: I256) -> Option<I256> {
    a.checked_mul(b)
}

/// x_n и a_n исходника (L61-L86): x0, x1 — без десятичных (e^x в целых), x2…x11 — 20 знаков.
fn table() -> [(I256, I256); 12] {
    [
        (
            c("128000000000000000000"),
            c("38877084059945950922200000000000000000000000000000000000"),
        ),
        (c("64000000000000000000"), c("6235149080811616882910000000")),
        (
            c("3200000000000000000000"),
            c("7896296018268069516100000000000000"),
        ),
        (
            c("1600000000000000000000"),
            c("888611052050787263676000000"),
        ),
        (c("800000000000000000000"), c("298095798704172827474000")),
        (c("400000000000000000000"), c("5459815003314423907810")),
        (c("200000000000000000000"), c("738905609893065022723")),
        (c("100000000000000000000"), c("271828182845904523536")),
        (c("50000000000000000000"), c("164872127070012814685")),
        (c("25000000000000000000"), c("128402541668774148407")),
        (c("12500000000000000000"), c("113314845306682631683")),
        (c("6250000000000000000"), c("106449445891785942956")),
    ]
}

/// LogExpMath.pow (L93-L139): x^y, оба — фикс-точка 18 знаков. Возвращает: результат или None — как откат
/// (X_OUT_OF_BOUNDS, Y_OUT_OF_BOUNDS, PRODUCT_OUT_OF_BOUNDS) или переполнение. Ошибок иных нет.
pub fn pow(x: U256, y: U256) -> Option<U256> {
    let one18 = ten(18);
    if y.is_zero() {
        return Some(one18.into_raw());
    }
    if x.is_zero() {
        return Some(U256::ZERO);
    }
    // x < 2^255 (L108); y < MILD_EXPONENT_BOUND = 2^254 / ONE_20 (L58, L115).
    if x.bit(255) || y >= (U256::from(1u8) << 254) / ten(20).into_raw() {
        return None;
    }
    let (xi, yi) = (I256::from_raw(x), I256::from_raw(y));
    let lower = one18 - ten(17);
    let upper = one18 + ten(17);
    let mut lxy = if lower < xi && xi < upper {
        let l = ln_36(xi)?;
        mul(sdiv(l, one18)?, yi)?.checked_add(sdiv(mul(smod(l, one18)?, yi)?, one18)?)?
    } else {
        mul(ln(xi)?, yi)?
    };
    lxy = sdiv(lxy, one18)?;
    // MIN_NATURAL_EXPONENT = −41e18, MAX_NATURAL_EXPONENT = 130e18 (L50-L51).
    if lxy < mul(c("-41"), one18)? || lxy > mul(c("130"), one18)? {
        return None;
    }
    Some(exp(lxy)?.into_raw())
}

/// LogExpMath.exp (L146-L281): e^x, x — фикс-точка 18 знаков в [MIN, MAX]_NATURAL_EXPONENT. None — вне пределов или
/// переполнение.
fn exp(mut x: I256) -> Option<I256> {
    let (one18, one20) = (ten(18), ten(20));
    if x < mul(c("-41"), one18)? || x > mul(c("130"), one18)? {
        return None;
    }
    if x.is_negative() {
        return sdiv(mul(one18, one18)?, exp(-x)?);
    }
    let t = table();
    let first = if x >= t[0].0 {
        x -= t[0].0;
        t[0].1
    } else if x >= t[1].0 {
        x -= t[1].0;
        t[1].1
    } else {
        I256::ONE
    };
    x = mul(x, c("100"))?;
    let mut product = one20;
    for (xn, an) in t.iter().take(10).skip(2) {
        if x >= *xn {
            x -= *xn;
            product = sdiv(mul(product, *an)?, one20)?;
        }
    }
    let mut sum = one20;
    let mut term = x;
    sum += term;
    for n in 2..=12u8 {
        term = sdiv(sdiv(mul(term, x)?, one20)?, I256::try_from(n).ok()?)?;
        sum += term;
    }
    sdiv(mul(sdiv(mul(product, sum)?, one20)?, first)?, c("100"))
}

/// LogExpMath.ln (L286-L421): натуральный логарифм, a > 0 — фикс-точка 18 знаков. None — a ≤ 0 или переполнение.
fn ln(mut a: I256) -> Option<I256> {
    let (one18, one20) = (ten(18), ten(20));
    if !a.is_positive() {
        return None;
    }
    if a < one18 {
        return Some(-ln(sdiv(mul(one18, one18)?, a)?)?);
    }
    let t = table();
    let mut sum = I256::ZERO;
    for (xn, an) in t.iter().take(2) {
        if a >= mul(*an, one18)? {
            a = sdiv(a, *an)?;
            sum += *xn;
        }
    }
    sum = mul(sum, c("100"))?;
    a = mul(a, c("100"))?;
    for (xn, an) in t.iter().skip(2) {
        if a >= *an {
            a = sdiv(mul(a, one20)?, *an)?;
            sum += *xn;
        }
    }
    let z = sdiv(mul(a - one20, one20)?, a + one20)?;
    let z2 = sdiv(mul(z, z)?, one20)?;
    let mut num = z;
    let mut series = num;
    for d in [3u8, 5, 7, 9, 11] {
        num = sdiv(mul(num, z2)?, one20)?;
        series += sdiv(num, I256::try_from(d).ok()?)?;
    }
    series = mul(series, c("2"))?;
    sdiv(sum + series, c("100"))
}

/// LogExpMath.ln_36 (L456-L503): логарифм с 36 знаками для x около 1 (LN_36_LOWER/UPPER_BOUND, L55-L56). None —
/// переполнение.
fn ln_36(x: I256) -> Option<I256> {
    let one36 = ten(36);
    let x = mul(x, ten(18))?;
    let z = sdiv(mul(x - one36, one36)?, x + one36)?;
    let z2 = sdiv(mul(z, z)?, one36)?;
    let mut num = z;
    let mut series = num;
    for d in [3u8, 5, 7, 9, 11, 13, 15] {
        num = sdiv(mul(num, z2)?, one36)?;
        series += sdiv(num, I256::try_from(d).ok()?)?;
    }
    mul(series, c("2"))
}
