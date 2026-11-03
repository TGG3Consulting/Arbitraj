//! Файл: venues/bsc_dex/math/stableswap_ng.rs
//! Что делает: математика обмена пула PancakeSwap Infinity Stable (крючок CLStableSwapHook — порт Curve StableSwap-NG,
//! решение «122 (Infinity Stable)») на целых U256 с теми же округлениями: `_dynamic_fee`, `get_D`, `get_y` и выход
//! `_exchange_received` (то, что крючок отдаёт в обмене через CLPoolManager.swap → beforeSwap). Отличия от шаблонов
//! math/stableswap_v1.rs: (1) комиссия ДИНАМИЧЕСКАЯ — растёт, когда пул уходит от привязки (offpeg_fee_multiplier);
//! (2) в get_D произведение D_P делится на каждый x, а на N^N — один раз в конце (у Curve 2020 — на x·N каждым шагом:
//! округления разные); (3) D считается один раз до get_y и передаётся в него; (4) несходимость за 255 раундов
//! откатывает обмен (`raise`) → здесь None.
//! Кто вызывает: quote/pool_quote.rs (ветка StableSwap с шаблоном NG); тесты.
//! От чего зависит: math/stableswap_v1.rs (xp, precision, fee_denominator), alloy-primitives (U256).
//! Настройки: нет (балансы, rates, A, fee, offpeg — с крючка на блоке, pools/load_inf_stable.rs).
//!
//! Источник (официальный: verified-код BscScan, решение 23): CLStableSwapHook 0x86c3DC08FB5a6663cCa15551575d5429e5Efc017
//! https://bscscan.com/address/0x86c3DC08FB5a6663cCa15551575d5429e5Efc017#code (Vyper 0.3.10, sha256
//! 28031f9aee0da15163a6e1fbc2841d6fcf3a55900e3c322975359d6f6024f17b): PRECISION L140, FEE_DENOMINATOR L148,
//! A_PRECISION L156, `_xp_mem` L944-L954, `_dynamic_fee` L617-L632, `_exchange_received` L635-L721 (выход L678-L695),
//! `get_y` L744-L811, `get_D` L814-L862. Vyper 0.3: обычная арифметика checked (переполнение, недобор, деление на ноль
//! откатываются → None); `unsafe_*` — без проверки (здесь wrapping_* / прямое деление, как в контракте).

use alloy_primitives::U256;

use super::stableswap_v1::{fee_denominator, precision, xp};

/// A_PRECISION = 100 (L156): A крючка хранится и применяется как A·100 (`A_precise()` L1267-L1270).
pub const A_PRECISION: u64 = 100;
/// Предел раундов Ньютона в get_D и get_y (`range(255)` L801, L835).
pub const MAX_ROUNDS: u16 = 255;

/// `_dynamic_fee(xpi, xpj, _fee)` (L617-L632): при offpeg ≤ 1e10 — постоянная fee; иначе
/// `offpeg·fee / ((offpeg − 1e10)·4·xpi·xpj/(xpi + xpj)² + 1e10)`. Принимает: xpi, xpj (18 знаков), fee и offpeg (над
/// 10^10). None — переполнение или деление на ноль (контракт откатился бы).
pub fn dynamic_fee(xpi: U256, xpj: U256, fee: U256, offpeg: U256) -> Option<U256> {
    let den = fee_denominator();
    if offpeg <= den {
        return Some(fee);
    }
    // xps2 = (xpi + xpj) ** 2 — checked.
    let sum = xpi.checked_add(xpj)?;
    let xps2 = sum.checked_mul(sum)?;
    if xps2.is_zero() {
        return None;
    }
    // unsafe_sub(offpeg, 1e10) · 4 · xpi · xpj / xps2 — умножения и деление checked, вычитание unsafe (offpeg > 1e10).
    let part = offpeg
        .wrapping_sub(den)
        .checked_mul(U256::from(4u8))?
        .checked_mul(xpi)?
        .checked_mul(xpj)?
        / xps2;
    // unsafe_div(unsafe_mul(offpeg, fee), unsafe_add(part, 1e10)).
    let d = part.wrapping_add(den);
    if d.is_zero() {
        return None;
    }
    Some(offpeg.wrapping_mul(fee) / d)
}

/// `get_D(_xp, _amp)` (L814-L862): инвариант D (сходимость |ΔD| ≤ 1). Принимает: xp (18 знаков), A·A_PRECISION.
/// Возвращает: D (0 при S = 0). None — деление на ноль, недобор, переполнение, несходимость за 255 раундов (raise).
pub fn get_d(xp: &[U256], amp: U256) -> Option<U256> {
    let n = U256::from(xp.len());
    let ap = U256::from(A_PRECISION);
    let mut s = U256::ZERO;
    for x in xp {
        s = s.checked_add(*x)?;
    }
    if s.is_zero() {
        return Some(U256::ZERO);
    }
    let mut d = s;
    let ann = amp.checked_mul(n)?;
    // pow_mod256(N_COINS, N_COINS) (L840): N ≤ 2 у крючка (MAX_COINS L133) — без переполнения.
    let nn = n.pow(n);
    let one = U256::from(1u8);
    for _ in 0..MAX_ROUNDS {
        let mut d_p = d;
        for x in xp {
            if x.is_zero() {
                return None;
            }
            d_p = d_p.checked_mul(d)? / *x;
        }
        d_p /= nn;
        let prev = d;
        // (unsafe_div(Ann·S, AP) + D_P·N)·D / (unsafe_div((Ann − AP)·D, AP) + unsafe_add(N, 1)·D_P) (L844-L851).
        let num = (ann.checked_mul(s)? / ap)
            .checked_add(d_p.checked_mul(n)?)?
            .checked_mul(d)?;
        let den = (ann.checked_sub(ap)?.checked_mul(d)? / ap)
            .checked_add(n.wrapping_add(one).checked_mul(d_p)?)?;
        if den.is_zero() {
            return None;
        }
        d = num / den;
        let step = if d > prev { d - prev } else { prev - d };
        if step <= one {
            return Some(d);
        }
    }
    None
}

/// `get_y(i, j, x, xp, _amp, _D)` (L744-L811): новый баланс монеты j (18 знаков), если баланс монеты i стал x, при
/// заданном D. Принимает: индексы, x, xp, A·A_PRECISION, D. None — неверные индексы (assert L765-L771), деление на
/// ноль, недобор, переполнение, несходимость за 255 раундов (raise L811).
pub fn get_y(i: usize, j: usize, x: U256, xp: &[U256], amp: U256, d: U256) -> Option<U256> {
    let n_len = xp.len();
    if i == j || i >= n_len || j >= n_len {
        return None;
    }
    let n = U256::from(n_len);
    let ap = U256::from(A_PRECISION);
    let ann = amp.checked_mul(n)?;
    let mut c = d;
    let mut s = U256::ZERO;
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
    // c = c·D·A_PRECISION/(Ann·N); b = S_ + D·A_PRECISION/Ann (L797-L798).
    let den = ann.checked_mul(n)?;
    if den.is_zero() {
        return None;
    }
    c = c.checked_mul(d)?.checked_mul(ap)? / den;
    let b = s.checked_add(d.checked_mul(ap)? / ann)?;
    let one = U256::from(1u8);
    let mut y = d;
    for _ in 0..MAX_ROUNDS {
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
    None
}

/// Выход `_exchange_received` (L662-L695): dx монеты i → монета j. Принимает: i, j, dx (сколько пришло в крючок —
/// ровно сумма обмена: beforeSwap L1700 чеканит крючку dx), балансы (`balances(k)` = stored − admin, L299-L320), rates
/// (`stored_rates()` на блоке, L259-L296), A·A_PRECISION (`A_precise()`), fee и offpeg (над 10^10). Возвращает:
/// (выход в единицах монеты j, dy_fee в 18 знаках). None — арифметика откатилась бы (в т. ч. dx = 0, assert L660).
#[allow(clippy::too_many_arguments)]
pub fn exchange_received(
    i: usize,
    j: usize,
    dx: U256,
    balances: &[U256],
    rates: &[U256],
    amp: U256,
    fee: U256,
    offpeg: U256,
) -> Option<(U256, U256)> {
    if i == j || dx.is_zero() {
        return None;
    }
    let p = precision();
    let xp = xp(balances, rates)?;
    let (xi, xj) = (*xp.get(i)?, *xp.get(j)?);
    // x = _xp[i] + unsafe_div(dx·rates[i], PRECISION) (L678).
    let x = xi.checked_add(dx.checked_mul(*rates.get(i)?)? / p)?;
    let d = get_d(&xp, amp)?;
    let y = get_y(i, j, x, &xp, amp, d)?;
    // dy = _xp[j] − y − 1 (L686).
    let dy = xj.checked_sub(y)?.checked_sub(U256::from(1u8))?;
    // dy_fee = unsafe_div(dy · _dynamic_fee(unsafe_div(xp[i] + x, 2), unsafe_div(xp[j] + y, 2), fee), FEE_DEN) (L687-L692).
    let two = U256::from(2u8);
    let f = dynamic_fee(
        xi.checked_add(x)? / two,
        xj.checked_add(y)? / two,
        fee,
        offpeg,
    )?;
    let dy_fee = dy.checked_mul(f)? / fee_denominator();
    // dy = (dy − dy_fee)·PRECISION / rates[j] (L695).
    let rj = *rates.get(j)?;
    if rj.is_zero() {
        return None;
    }
    Some((dy.checked_sub(dy_fee)?.checked_mul(p)? / rj, dy_fee))
}
