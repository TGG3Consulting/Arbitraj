//! Файл: venues/bsc_dex/math/stableswap_v1.rs
//! Что делает: математика пула StableSwap шаблона Curve 2020 (Vyper 0.2.x, без A_PRECISION) — Ellipsis StableSwap
//! (решение 122 F) — порт `get_D`, `get_y` и обмена `exchange` на целых U256 с теми же округлениями. Отличия от
//! шаблона SwapTemplateBase (math/curve_math.rs): `A()` — «сырой» A (без множителя A_PRECISION), формулы D и y —
//! без деления на A_PRECISION, и НЕТ отката при несходимости за 255 раундов (возвращается последнее значение).
//! Выход обмена — как в `exchange` (то, что пул реально переводит): `(dy − dy·fee/1e10)·1e18/rates[j]`, где
//! `dy = xp[j] − y − 1`. Квотер пула `get_dy` считает комиссию после перевода в единицы монеты — при rates = 1e18
//! (все монеты 18 знаков) результаты совпадают, иначе расходятся не больше чем на округление.
//! Кто вызывает: quote/pool_quote.rs, quote/depth_band.rs, quote/spot.rs (ветка StableSwap); тесты.
//! От чего зависит: alloy-primitives (U256). Настройки: нет (балансы, rates, A, fee — с контракта пула).
//!
//! Источник (официальный, решение 23): ellipsis-finance/ellipsis (HEAD 6bdf7788020695b72034f5c9f1fda11c5e3cefb7),
//! contracts/StableSwap.vy (@version 0.2.11, «(c) Curve.Fi, 2020»): `_xp_mem` L189-L195, `get_D` L198-L223, `get_y`
//! L359-L402, `get_dy` L405-L416, `exchange` L434-L470. Vyper 0.2 — арифметика checked: переполнение, недобор и
//! деление на ноль откатываются → здесь `None`.

use alloy_primitives::U256;

/// PRECISION = LENDING_PRECISION = 10^18 (StableSwap.vy L81-L82). Ошибок нет.
pub fn precision() -> U256 {
    U256::from(10u8).pow(U256::from(18u8))
}

/// FEE_DENOMINATOR = 10^10 (StableSwap.vy L80). Ошибок нет.
pub fn fee_denominator() -> U256 {
    U256::from(10u8).pow(U256::from(10u8))
}

/// `_xp_mem`: балансы в 18 знаках — `rates[k]·balances[k]/1e18`. None — длины не совпадают или переполнение.
pub fn xp(balances: &[U256], rates: &[U256]) -> Option<Vec<U256>> {
    if balances.len() != rates.len() {
        return None;
    }
    let p = precision();
    balances
        .iter()
        .zip(rates)
        .map(|(b, r)| Some(r.checked_mul(*b)? / p))
        .collect()
}

/// `get_D`: инвариант D итерацией Ньютона (до 255 раундов, сходимость |ΔD| ≤ 1, без отката). Принимает: xp, A.
/// Возвращает: D (0 при пустом пуле). None — деление на ноль (нулевой баланс), недобор, переполнение.
pub fn get_d(xp: &[U256], amp: U256) -> Option<U256> {
    let n = U256::from(xp.len());
    let mut s = U256::ZERO;
    for x in xp {
        s = s.checked_add(*x)?;
    }
    if s.is_zero() {
        return Some(U256::ZERO);
    }
    let one = U256::from(1u8);
    let mut d = s;
    let ann = amp.checked_mul(n)?;
    for _ in 0..255u16 {
        let mut d_p = d;
        for x in xp {
            let den = x.checked_mul(n)?;
            if den.is_zero() {
                return None;
            }
            d_p = d_p.checked_mul(d)? / den;
        }
        let prev = d;
        let num = ann
            .checked_mul(s)?
            .checked_add(d_p.checked_mul(n)?)?
            .checked_mul(d)?;
        let den = ann
            .checked_sub(one)?
            .checked_mul(d)?
            .checked_add(n.checked_add(one)?.checked_mul(d_p)?)?;
        if den.is_zero() {
            return None;
        }
        d = num / den;
        let step = if d > prev { d - prev } else { prev - d };
        if step <= one {
            break;
        }
    }
    Some(d)
}

/// `get_y`: новый баланс монеты j (в 18 знаках), если баланс монеты i стал x. До 255 раундов, без отката.
/// Принимает: i, j, x, xp, A. None — неверные индексы, деление на ноль, недобор, переполнение.
pub fn get_y(i: usize, j: usize, x: U256, xp: &[U256], amp: U256) -> Option<U256> {
    let n_len = xp.len();
    if i == j || i >= n_len || j >= n_len {
        return None;
    }
    let n = U256::from(n_len);
    let d = get_d(xp, amp)?;
    let mut c = d;
    let mut s = U256::ZERO;
    let ann = amp.checked_mul(n)?;
    for (k, xk) in xp.iter().enumerate() {
        let v = if k == i {
            x
        } else if k != j {
            *xk
        } else {
            continue;
        };
        s = s.checked_add(v)?;
        let den = v.checked_mul(n)?;
        if den.is_zero() {
            return None;
        }
        c = c.checked_mul(d)? / den;
    }
    let den = ann.checked_mul(n)?;
    if den.is_zero() || ann.is_zero() {
        return None;
    }
    c = c.checked_mul(d)? / den;
    let b = s.checked_add(d / ann)?;
    let one = U256::from(1u8);
    let mut y = d;
    for _ in 0..255u16 {
        let prev = y;
        let den = U256::from(2u8)
            .checked_mul(y)?
            .checked_add(b)?
            .checked_sub(d)?;
        if den.is_zero() {
            return None;
        }
        y = y.checked_mul(y)?.checked_add(c)? / den;
        let step = if y > prev { y - prev } else { prev - y };
        if step <= one {
            break;
        }
    }
    Some(y)
}

/// `exchange` (выход, который пул переводит): dx монеты i → монета j. Принимает: i, j, dx, балансы, rates, A,
/// fee (над 10^10). Возвращает: выход в единицах монеты j. None — как у get_y, недобор `xp[j] − y − 1`, rates = 0.
pub fn exchange_out(
    i: usize,
    j: usize,
    dx: U256,
    balances: &[U256],
    rates: &[U256],
    amp: U256,
    fee: U256,
) -> Option<U256> {
    let xp = xp(balances, rates)?;
    let p = precision();
    let x = xp
        .get(i)?
        .checked_add(dx.checked_mul(*rates.get(i)?)? / p)?;
    let y = get_y(i, j, x, &xp, amp)?;
    let dy = xp.get(j)?.checked_sub(y)?.checked_sub(U256::from(1u8))?;
    let dy_fee = dy.checked_mul(fee)? / fee_denominator();
    let rj = *rates.get(j)?;
    if rj.is_zero() {
        return None;
    }
    Some(dy.checked_sub(dy_fee)?.checked_mul(p)? / rj)
}

/// Множитель точности монеты по её decimals — правило Curve `RATES[k] = 10^(36 − decimals)` (у Ellipsis 3pool все
/// монеты 18 знаков → 1e18, StableSwap.vy L84). None — decimals > 36. Ошибок нет.
pub fn rate_for_decimals(decimals: u8) -> Option<U256> {
    let e = 36u8.checked_sub(decimals)?;
    Some(U256::from(10u8).pow(U256::from(e)))
}
