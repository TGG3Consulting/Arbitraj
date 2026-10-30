//! Файл: venues/bsc_dex/math/dodo_math.rs
//! Что делает: базовая математика кривой DODO PMM (решение 80 — экзотическая математика DODO/PMM) — порт официальных
//! библиотек контрактов DODO V2 DecimalMath (фикс-точка 18 знаков) и DODOMath (интеграл кривой и два решения
//! квадратного уравнения) на целых числах U256, с теми же округлениями и теми же местами переполнения, что в Solidity.
//! Это чистая математика без сети; PMM-кривая (разбор R-состояний, покупка/продажа) — в math/swap_dodo.rs.
//! ВАЖНО (решение 23): порт следует коду контракта дословно, включая различие «сырого» умножения (в Solidity 0.6.9
//! оператор `*` переполняется по модулю 2^256 — DODO этим ловит переполнение вручную проверкой `(a*b)/a == b`) и
//! умножения через SafeMath (`.mul`, откат при переполнении). Сырое → wrapping_mul, SafeMath → checked_* (None там,
//! где контракт откатился бы: переполнение, вычитание ниже нуля, деление на ноль).
//! Охват (рекомендация техлида): это V2 PMM — пулы DVM / DPP / DSP, которые ценятся on-chain. Classical V1 (цена с
//! оракула Chainlink) этой библиотекой не считается и в проект не берётся.
//! Кто вызывает: math/swap_dodo.rs (PMM-кривая); тесты (tests/bsc_dex_dodo_math.rs) — через переэкспорт в swap_dodo.
//! От чего зависит: alloy-primitives (U256). Настройки: нет (i, K передаются вызывающим — читаются с контракта в 86b).
//!
//! Источники (официальные, решение 23): DODOEX/contractV2 (HEAD 2f1bcda):
//! - DecimalMath (ONE=10^18, ONE2=10^36, mulFloor/mulCeil/divFloor/divCeil/reciprocalFloor) — contracts/lib/DecimalMath.sol;
//! - DODOMath._GeneralIntegrate / _SolveQuadraticFunctionForTarget / _SolveQuadraticFunctionForTrade — contracts/lib/DODOMath.sol;
//! - SafeMath.sqrt (вавилонский, floor) и SafeMath.divCeil — contracts/lib/SafeMath.sol.

use alloy_primitives::U256;

/// DecimalMath.ONE = 10^18 (единица фикс-точки на 18 знаков). Ошибок нет.
pub fn one() -> U256 {
    U256::from(10u8).pow(U256::from(18u8))
}

/// DecimalMath.ONE2 = 10^36. Ошибок нет.
pub fn one2() -> U256 {
    U256::from(10u8).pow(U256::from(36u8))
}

/// SafeMath.sqrt: целочисленный квадратный корень вниз (вавилонский метод, ровно как в контракте). Ошибок нет.
pub fn sqrt(x: U256) -> U256 {
    let two = U256::from(2u8);
    let mut z = x / two + U256::from(1u8);
    let mut y = x;
    while z < y {
        y = z;
        z = (x / z + z) / two;
    }
    y
}

/// SafeMath.divCeil: ceil(a / b). None — b = 0 (контракт откатился бы в div).
pub fn div_ceil(a: U256, b: U256) -> Option<U256> {
    if b.is_zero() {
        return None;
    }
    let q = a / b;
    Some(if (a - q * b).is_zero() {
        q
    } else {
        q + U256::from(1u8)
    })
}

/// DecimalMath.mulFloor: floor(target·d / 10^18). None — target·d > 2^256−1 (SafeMath.mul откатился бы).
pub fn mul_floor(target: U256, d: U256) -> Option<U256> {
    Some(target.checked_mul(d)? / one())
}

/// DecimalMath.mulCeil: ceil(target·d / 10^18). None — переполнение target·d.
pub fn mul_ceil(target: U256, d: U256) -> Option<U256> {
    div_ceil(target.checked_mul(d)?, one())
}

/// DecimalMath.divFloor: floor(target·10^18 / d). None — переполнение target·10^18 или d = 0.
pub fn div_floor(target: U256, d: U256) -> Option<U256> {
    let num = target.checked_mul(one())?;
    if d.is_zero() {
        return None;
    }
    Some(num / d)
}

/// DecimalMath.reciprocalFloor: floor(10^36 / target). None — target = 0.
pub fn reciprocal_floor(target: U256) -> Option<U256> {
    if target.is_zero() {
        return None;
    }
    Some(one2() / target)
}

/// DODOMath._GeneralIntegrate: интеграл кривой DODO от V1 до V2 (res = i·delta·(1−k+k·V0²/V1/V2)), округление вниз.
/// Принимает: V0 (цель), V1 ≥ V2 (границы), i (цена пары), k (наклон). Возвращает: сумму встречного актива.
/// None — V0 = 0 (контракт: require TARGET_IS_ZERO) или переполнение/недобор любой из операций SafeMath.
pub fn general_integrate(v0: U256, v1: U256, v2: U256, i: U256, k: U256) -> Option<U256> {
    if v0.is_zero() {
        return None;
    }
    let fair_amount = i.checked_mul(v1.checked_sub(v2)?)?; // i·delta
    if k.is_zero() {
        return Some(fair_amount / one());
    }
    // V0²/V1/V2 = divFloor(V0·V0/V1, V2)
    let v0v0v1 = v0.checked_mul(v0)? / nz(v1)?;
    let v0v0v1v2 = div_floor(v0v0v1, v2)?;
    let penalty = mul_floor(k, v0v0v1v2)?; // k·V0²/V1/V2
    // (1−k+penalty)·fairAmount / 10^36
    let factor = one().checked_sub(k)?.checked_add(penalty)?;
    Some(factor.checked_mul(fair_amount)? / one2())
}

/// DODOMath._SolveQuadraticFunctionForTarget: по V1, delta, i, k найти V0 (цель). Округление вниз.
/// None — переполнение SafeMath или деление на ноль. (V1 = 0 → 0, как в контракте.)
pub fn solve_quadratic_for_target(v1: U256, delta: U256, i: U256, k: U256) -> Option<U256> {
    if k.is_zero() {
        return v1.checked_add(mul_floor(i, delta)?);
    }
    if v1.is_zero() {
        return Some(U256::ZERO);
    }
    // ki = (4·k)·i  — «4·k» сырое (не переполняется при k ≤ 10^18), «·i» через SafeMath.
    let ki = (U256::from(4u8) * k).checked_mul(i)?;
    let sqrt_v = if ki.is_zero() {
        one()
    } else if (ki.wrapping_mul(delta)) / ki == delta {
        // Нет переполнения ki·delta (сырое) → (ki·delta)/V1 + ONE2, затем корень.
        sqrt(ki.wrapping_mul(delta) / v1 + one2())
    } else {
        // Переполнение ki·delta → (ki/V1)·delta + ONE2 (как в контракте: ki.div(V1).mul(delta)).
        sqrt((ki / v1).checked_mul(delta)?.checked_add(one2())?)
    };
    // premium = divFloor(sqrt−1, 2k) + 1; V0 = mulFloor(V1, premium).
    let premium = div_floor(sqrt_v.checked_sub(one())?, k.checked_mul(U256::from(2u8))?)?
        .checked_add(one())?;
    mul_floor(v1, premium)
}

/// DODOMath._SolveQuadraticFunctionForTrade: по V0, V1, delta, i, k найти |V1−V2| (сколько получит трейдер).
/// Округление вниз. None — V0 = 0 (require TARGET_IS_ZERO), переполнение/недобор SafeMath, или ветка «не ноль» дала 0
/// (контракт: revert "DODOMath: should not be zero"). delta = 0 → 0; V2 > V1 → 0 (как в контракте).
pub fn solve_quadratic_for_trade(
    v0: U256,
    v1: U256,
    delta: U256,
    i: U256,
    k: U256,
) -> Option<U256> {
    if v0.is_zero() {
        return None;
    }
    if delta.is_zero() {
        return Some(U256::ZERO);
    }
    if k.is_zero() {
        let idelta = mul_floor(i, delta)?;
        return Some(if idelta > v1 { v1 } else { idelta });
    }
    if k == one() {
        // Q1−Q2 = Q1·temp/(1+temp), temp = i·delta·Q1/Q0².
        let idelta = i.checked_mul(delta)?; // SafeMath.mul
        let temp = if idelta.is_zero() {
            U256::ZERO
        } else if idelta.wrapping_mul(v1) / idelta == v1 {
            idelta.wrapping_mul(v1) / nz(v0.checked_mul(v0)?)?
        } else {
            (delta.checked_mul(v1)? / nz(v0)?).checked_mul(i)? / nz(v0)?
        };
        return Some(v1.checked_mul(temp)? / nz(temp.checked_add(one())?)?);
    }
    // part2 = k·V0/V1·V0 + i·delta ; bAbs = (1−k)·V1 ; знак b.
    let part2 = (k.checked_mul(v0)? / nz(v1)?)
        .checked_mul(v0)?
        .checked_add(i.checked_mul(delta)?)?;
    let mut b_abs = one().checked_sub(k)?.checked_mul(v1)?;
    let b_sig = b_abs < part2;
    b_abs = if b_sig { part2 - b_abs } else { b_abs - part2 };
    b_abs /= one();
    // squareRoot = sqrt(bAbs² + 4(1−k)·k·V0²).
    let four_part = mul_floor(
        one().checked_sub(k)?.checked_mul(U256::from(4u8))?,
        mul_floor(k, v0)?.checked_mul(v0)?,
    )?;
    let square_root = sqrt(b_abs.checked_mul(b_abs)?.checked_add(four_part)?);
    let denominator = one().checked_sub(k)?.checked_mul(U256::from(2u8))?;
    let numerator = if b_sig {
        let n = square_root.checked_sub(b_abs)?;
        if n.is_zero() {
            return None; // revert "DODOMath: should not be zero"
        }
        n
    } else {
        b_abs.checked_add(square_root)?
    };
    let v2 = div_ceil(numerator.checked_mul(one())?, denominator)?; // DecimalMath.divCeil
    Some(if v2 > v1 { U256::ZERO } else { v1 - v2 })
}

/// Защита от деления на ноль: None — x = 0 (в контракте этот div откатился бы). Ошибок иначе нет.
fn nz(x: U256) -> Option<U256> {
    if x.is_zero() { None } else { Some(x) }
}
