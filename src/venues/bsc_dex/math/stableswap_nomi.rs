//! Файл: venues/bsc_dex/math/stableswap_nomi.rs
//! Что делает: решение 284 — порт расчёта выхода пары Nomiswap Stable (NomiswapStablePair, две монеты): D по Ньютону
//! (`_computeLiquidityFromAdjustedBalances`), y по Ньютону с округлением вверх (`_getY`, divRoundUp), выход
//! `getAmountOut` — комиссия снимается со ВХОДА, выход = (xp[j] − y) / множитель (без −1). Целые U256, те же
//! округления; переполнение / деление на 0 / недобор — None (у пары — откат или мусор unchecked: не торгуем).
//! Состояние — StableData (шаблон input_fee): балансы = резервы пары, rates = множитель·1e18, amp = getA() (A·100),
//! fee = swapFee·10^5 (над 10^10 — дробь та же, что swapFee/MAX_FEE: floor совпадает), amp_precision = 100,
//! max_rounds = 256 (MAX_LOOP_LIMIT пары), без отката при несходимости (цикл просто кончается).
//! Кто вызывает: quote/pool_quote.rs (ветка StableSwap, input_fee); тесты.
//! От чего зависит: math/stableswap_v1.rs (precision, fee_denominator), alloy-primitives.
//! Настройки: нет.
//!
//! Источник — verified-исходник Sourcify full match, chain 56, пара 0x1e40450f8e21bb68490d7d91ab422888fb3d60f1
//! (тот же NomiswapStablePair.sol у 0x3a667100…da87 и 0xca25f030…a7ec; solc 0.8.15): contracts/NomiswapStablePair.sol
//! `getAmountOut` L251-L274, `_computeLiquidityFromAdjustedBalances` L337-L355, `_getY` L357-L374;
//! contracts/libraries/MathUtils.sol `within1` L12-L19, `divRoundUp` L21-L23. Проверено на 24 ответах getAmountOut
//! трёх пар на блоке 133650877 (tests/bsc_dex_nomiswap_stable_live.rs).

use alloy_primitives::U256;

use super::stableswap_v1::{fee_denominator, precision};

/// |a − b| ≤ 1 (MathUtils.within1 L12-L19). Ошибок нет.
fn within1(a: U256, b: U256) -> bool {
    let d = if a > b { a - b } else { b - a };
    d <= U256::from(1u8)
}

/// D пары по приведённым балансам (L337-L355): N_A = A·4, D_P = D³/x0/x1/4, раунды до within1 или max_rounds.
/// Принимает: xp0, xp1, A (A·точность), точность A, предел раундов. Возвращает: D; None — x = 0, A·4 < точности,
/// переполнение, деление на 0.
pub fn get_d(xp0: U256, xp1: U256, amp: U256, ap: U256, rounds: u16) -> Option<U256> {
    let s = xp0.checked_add(xp1)?;
    if s.is_zero() {
        return Some(U256::ZERO);
    }
    let n_a = amp.checked_mul(U256::from(4u8))?;
    let three = U256::from(3u8);
    let mut d = s;
    for _ in 0..rounds {
        let dp = d
            .checked_mul(d)?
            .checked_div(xp0)?
            .checked_mul(d)?
            .checked_div(xp1)?
            / U256::from(4u8);
        let prev = d;
        let num =
            (n_a.checked_mul(s)?.checked_div(ap)?).checked_add(dp.checked_mul(U256::from(2u8))?)?;
        let den = (n_a.checked_div(ap)?.checked_sub(U256::from(1u8))?)
            .checked_mul(d)?
            .checked_add(three.checked_mul(dp)?)?;
        d = num.checked_mul(d)?.checked_div(den)?;
        if within1(d, prev) {
            break;
        }
    }
    Some(d)
}

/// y при заданном x и D (L357-L374): c = D²/(2x)·D/(2·N_A/точность), b = x + D·точность/N_A,
/// y = ⌈(y² + c)/(2y + b − D)⌉. Принимает: x, D, A, точность A, предел раундов. Возвращает: y; None — как у get_d.
pub fn get_y(x: U256, d: U256, amp: U256, ap: U256, rounds: u16) -> Option<U256> {
    let two = U256::from(2u8);
    let n_a = amp.checked_mul(U256::from(4u8))?;
    let c = d.checked_mul(d)?.checked_div(x.checked_mul(two)?)?;
    let c = c
        .checked_mul(d)?
        .checked_div(n_a.checked_mul(two)?.checked_div(ap)?)?;
    let b = x.checked_add(d.checked_mul(ap)?.checked_div(n_a)?)?;
    let mut y = d;
    for _ in 0..rounds {
        let prev = y;
        let num = y.checked_mul(y)?.checked_add(c)?;
        let den = y.checked_mul(two)?.checked_add(b)?.checked_sub(d)?;
        if den.is_zero() {
            return None;
        }
        y = num.checked_add(den - U256::from(1u8))? / den;
        if within1(y, prev) {
            break;
        }
    }
    Some(y)
}

/// Выход getAmountOut (L251-L274): вход i → монета j пары. Принимает: i, j (0/1), вход, балансы (резервы), rates
/// (множитель·1e18), A, fee (над 10^10), точность A, предел раундов. Возвращает: выход в единицах монеты j; None —
/// не две монеты, множитель 0, переполнение, y > xp[j] (у пары unchecked дал бы мусор — обмен не пройдёт).
#[allow(clippy::too_many_arguments)]
pub fn amount_out(
    i: usize,
    j: usize,
    dx: U256,
    balances: &[U256],
    rates: &[U256],
    amp: U256,
    fee: U256,
    ap: U256,
    rounds: u16,
) -> Option<U256> {
    if balances.len() != 2 || rates.len() != 2 || i > 1 || j > 1 || i == j {
        return None;
    }
    let p = precision();
    let m = |k: usize| rates[k] / p;
    let (mi, mj) = (m(i), m(j));
    if mi.is_zero() || mj.is_zero() {
        return None;
    }
    let den = fee_denominator();
    let net = dx.checked_mul(den.checked_sub(fee)?)? / den;
    let xp0 = balances[0].checked_mul(m(0))?;
    let xp1 = balances[1].checked_mul(m(1))?;
    let d = get_d(xp0, xp1, amp, ap, rounds)?;
    let (xi, xj) = if i == 0 { (xp0, xp1) } else { (xp1, xp0) };
    let x = xi.checked_add(net.checked_mul(mi)?)?;
    let y = get_y(x, d, amp, ap, rounds)?;
    Some(xj.checked_sub(y)? / mj)
}
