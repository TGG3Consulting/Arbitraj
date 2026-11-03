//! Файл: venues/bsc_dex/math/stableswap_v1.rs
//! Что делает: математика пула StableSwap шаблона Curve 2020 (Vyper 0.2.x, без A_PRECISION) — Ellipsis StableSwap
//! (решение 122 F) — порт `get_D`, `get_y` и обмена `exchange` на целых U256 с теми же округлениями. Решение 122 F, 3:
//! те же формулы с параметрами шаблона `Template` — точность A (1 или A_PRECISION = 100), предел раундов Ньютона и
//! «откат при несходимости» — покрывают Lista StableSwap (Solidity-форк с A_PRECISION, 255 раундов без отката) и
//! Saddle (Nerve; A_PRECISION, 256 раундов, откат). При точности 1 формулы совпадают дословно с Curve 2020. Отличия от
//! шаблона SwapTemplateBase (math/curve_math.rs): `A()` — «сырой» A (без множителя A_PRECISION), формулы D и y —
//! без деления на A_PRECISION, и НЕТ отката при несходимости за 255 раундов (возвращается последнее значение).
//! Выход обмена — как в `exchange` (то, что пул реально переводит): `(dy − dy·fee/1e10)·1e18/rates[j]`, где
//! `dy = xp[j] − y − 1`. Квотер пула `get_dy` считает комиссию после перевода в единицы монеты — при rates = 1e18
//! (все монеты 18 знаков) результаты совпадают, иначе расходятся не больше чем на округление.
//! Кто вызывает: quote/pool_quote.rs, quote/depth_band.rs, quote/spot.rs (ветка StableSwap); тесты.
//! От чего зависит: alloy-primitives (U256). Настройки: нет (балансы, rates, A, fee — с контракта пула).
//!
//! Lista: lista-dao/moolah eb7d73a src/dex/StableSwapPool.sol — `A_PRECISION = 100` L41, `_xp_mem` L191-L196, `get_D`
//! L198-L231 (255 раундов, без отката), `get_y` L362-L403, `exchange` L478-L520 (выход как здесь).
//! Saddle (Nerve): nerve-finance/contracts bda7578 SwapUtils.sol — `MAX_LOOP_LIMIT = 256` L150, `A_PRECISION = 100`
//! L153, `getD` L393-L430 (откат «D does not converge»), `_xp` L458-L472 (balance·multiplier — равно
//! rate·balance/1e18 при rate = multiplier·1e18), `getY` L531-L582 (откат «Approximation did not converge»),
//! `_calculateSwap` L611-L630 ((dy − fee)/multiplier — равно (dy − fee)·1e18/rate).
//!
//! Источник (официальный, решение 23): ellipsis-finance/ellipsis (HEAD 6bdf7788020695b72034f5c9f1fda11c5e3cefb7),
//! contracts/StableSwap.vy (@version 0.2.11, «(c) Curve.Fi, 2020»): `_xp_mem` L189-L195, `get_D` L198-L223, `get_y`
//! L359-L402, `get_dy` L405-L416, `exchange` L434-L470. Vyper 0.2 — арифметика checked: переполнение, недобор и
//! деление на ноль откатываются → здесь `None`.

use alloy_primitives::U256;

pub use super::stableswap_exchange::{exchange_out, exchange_out_t, exchange_parts};

/// Параметры шаблона пула: точность A, предел раундов Ньютона, откат при несходимости. Ошибок нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Template {
    /// 1 — сырой A (Curve 2020), 100 — A·A_PRECISION.
    pub amp_precision: U256,
    /// Раундов итерации D и y (Curve 2020 и Lista — 255, Saddle — 256).
    pub max_rounds: u16,
    /// true — несошедшаяся итерация откатывает обмен (Saddle); false — берётся последнее значение.
    pub strict: bool,
}

/// Шаблон Curve 2020 (Ellipsis): сырой A, 255 раундов, без отката (StableSwap.vy L206, L382). Ошибок нет.
pub fn curve2020() -> Template {
    Template {
        amp_precision: U256::from(1u8),
        max_rounds: 255,
        strict: false,
    }
}

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

/// `get_D` шаблона Curve 2020 (см. get_d_t). Ошибок нет: None — как у get_d_t.
pub fn get_d(xp: &[U256], amp: U256) -> Option<U256> {
    get_d_t(xp, amp, &curve2020())
}

/// `get_D`: инвариант D итерацией Ньютона (сходимость |ΔD| ≤ 1). Принимает: xp, A (в точности шаблона), шаблон.
/// Возвращает: D (0 при пустом пуле). None — деление на ноль (нулевой баланс), недобор, переполнение, несходимость
/// у строгого шаблона.
pub fn get_d_t(xp: &[U256], amp: U256, t: &Template) -> Option<U256> {
    let n = U256::from(xp.len());
    let mut s = U256::ZERO;
    for x in xp {
        s = s.checked_add(*x)?;
    }
    if s.is_zero() {
        return Some(U256::ZERO);
    }
    let one = U256::from(1u8);
    let ap = t.amp_precision;
    if ap.is_zero() {
        return None;
    }
    let mut d = s;
    let ann = amp.checked_mul(n)?;
    for _ in 0..t.max_rounds {
        let mut d_p = d;
        for x in xp {
            let den = x.checked_mul(n)?;
            if den.is_zero() {
                return None;
            }
            d_p = d_p.checked_mul(d)? / den;
        }
        let prev = d;
        // (Ann·S/AP + D_P·N)·D / ((Ann − AP)·D/AP + (N+1)·D_P); при AP = 1 — формула Curve 2020 дословно.
        let num = (ann.checked_mul(s)? / ap)
            .checked_add(d_p.checked_mul(n)?)?
            .checked_mul(d)?;
        let den = (ann.checked_sub(ap)?.checked_mul(d)? / ap)
            .checked_add(n.checked_add(one)?.checked_mul(d_p)?)?;
        if den.is_zero() {
            return None;
        }
        d = num / den;
        let step = if d > prev { d - prev } else { prev - d };
        if step <= one {
            return Some(d);
        }
    }
    if t.strict { None } else { Some(d) }
}

/// `get_y` шаблона Curve 2020 (см. get_y_t). Ошибок нет: None — как у get_y_t.
pub fn get_y(i: usize, j: usize, x: U256, xp: &[U256], amp: U256) -> Option<U256> {
    get_y_t(i, j, x, xp, amp, &curve2020())
}

/// `get_y`: новый баланс монеты j (в 18 знаках), если баланс монеты i стал x. Принимает: i, j, x, xp, A, шаблон.
/// None — неверные индексы, деление на ноль, недобор, переполнение, несходимость у строгого шаблона.
pub fn get_y_t(i: usize, j: usize, x: U256, xp: &[U256], amp: U256, t: &Template) -> Option<U256> {
    let n_len = xp.len();
    if i == j || i >= n_len || j >= n_len {
        return None;
    }
    let n = U256::from(n_len);
    let ap = t.amp_precision;
    let d = get_d_t(xp, amp, t)?;
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
    // c = c·D·AP/(Ann·N); b = S + D·AP/Ann (при AP = 1 — Curve 2020 дословно).
    c = c.checked_mul(d)?.checked_mul(ap)? / den;
    let b = s.checked_add(d.checked_mul(ap)? / ann)?;
    let one = U256::from(1u8);
    let mut y = d;
    for _ in 0..t.max_rounds {
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
            return Some(y);
        }
    }
    if t.strict { None } else { Some(y) }
}

/// Множитель точности монеты по её decimals — правило Curve `RATES[k] = 10^(36 − decimals)` (у Ellipsis 3pool все
/// монеты 18 знаков → 1e18, StableSwap.vy L84). None — decimals > 36. Ошибок нет.
pub fn rate_for_decimals(decimals: u8) -> Option<U256> {
    let e = 36u8.checked_sub(decimals)?;
    Some(U256::from(10u8).pow(U256::from(e)))
}
