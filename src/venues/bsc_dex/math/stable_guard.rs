//! Файл: venues/bsc_dex/math/stable_guard.rs
//! Что делает: проверка цены пула StableSwap после обмена против внешнего оракула (решение 122 F, 3) — порт
//! `checkPriceDiff` и `get_dy_without_fee` Lista StableSwap на целых U256 с теми же округлениями. Пул откатывает
//! обмен, если после него цена любой из двух монет пула (по обмену на $100 в обе стороны) ушла от цены оракула
//! дальше порога. Котировка, которая уводит пул за порог, — неисполнима (такой обмен откатится).
//! Новые балансы после обмена — как в `exchange`: balances[i] + dx, balances[j] − dy − dy_admin_fee.
//! Кто вызывает: quote/pool_quote.rs (ветка StableSwap, у пула есть StableGuard); тесты.
//! От чего зависит: math/stableswap_v1.rs, alloy-primitives (U256), core/types (StableGuard).
//! Настройки: нет (цены оракула, пороги, admin_fee — с контракта пула на блоке).
//!
//! Источник (официальный, решение 23): lista-dao/moolah eb7d73a, src/dex/StableSwapPool.sol: `exchange` L478-L520
//! (dy_admin_fee L496-L497, новые балансы L500-L502, проверка после обновления балансов L504-L505),
//! `get_dy_without_fee` L405-L414, `fetchOraclePrice` L424-L430 (peek·1e10), `checkPriceDiff` L433-L462
//! (value = 100·1e18, dx = value·10^dps/цена, price0/price1, сравнение diff·1e18 ≤ цена·порог). Solidity 0.8 checked:
//! переполнение, недобор, деление на ноль откатываются → здесь `false` (обмен не пройдёт).

use alloy_primitives::U256;

use crate::core::types::StableGuard;

use super::stableswap_v1::{Template, exchange_parts, fee_denominator, get_y_t, precision, xp};

/// Пройдёт ли проверка цены после обмена dx монеты i → j. Принимает: индексы, вход, балансы, rates, A, fee, шаблон,
/// данные проверки. Возвращает: true — обмен не откатится по цене; false — откатится (или обмен невозможен).
#[allow(clippy::too_many_arguments)]
pub fn passes(
    i: usize,
    j: usize,
    dx: U256,
    balances: &[U256],
    rates: &[U256],
    amp: U256,
    fee: U256,
    t: &Template,
    g: &StableGuard,
) -> bool {
    after(i, j, dx, balances, rates, amp, fee, t, g)
        .and_then(|b| check(&b, rates, amp, t, g))
        .unwrap_or(false)
}

/// Балансы после обмена (L496-L502). None — обмен невозможен или арифметика откатилась бы.
#[allow(clippy::too_many_arguments)]
fn after(
    i: usize,
    j: usize,
    dx: U256,
    balances: &[U256],
    rates: &[U256],
    amp: U256,
    fee: U256,
    t: &Template,
    g: &StableGuard,
) -> Option<Vec<U256>> {
    let (dy, dy_fee) = exchange_parts(i, j, dx, balances, rates, amp, fee, t)?;
    let rj = *rates.get(j)?;
    if rj.is_zero() {
        return None;
    }
    let admin =
        (dy_fee.checked_mul(g.admin_fee)? / fee_denominator()).checked_mul(precision())? / rj;
    let mut b = balances.to_vec();
    b[i] = b.get(i)?.checked_add(dx)?;
    b[j] = b.get(j)?.checked_sub(dy)?.checked_sub(admin)?;
    Some(b)
}

/// `get_dy_without_fee` (L405-L414) на заданных балансах. None — откат.
fn dy_no_fee(
    i: usize,
    j: usize,
    dx: U256,
    b: &[U256],
    rates: &[U256],
    amp: U256,
    t: &Template,
) -> Option<U256> {
    let p = precision();
    let xp = xp(b, rates)?;
    let x = xp
        .get(i)?
        .checked_add(dx.checked_mul(*rates.get(i)?)? / p)?;
    let y = get_y_t(i, j, x, &xp, amp, t)?;
    let rj = *rates.get(j)?;
    if rj.is_zero() {
        return None;
    }
    Some(
        xp.get(j)?
            .checked_sub(y)?
            .checked_sub(U256::from(1u8))?
            .checked_mul(p)?
            / rj,
    )
}

/// `checkPriceDiff` (L433-L462) на балансах после обмена. Возвращает: Some(true/false); None — откат арифметики.
fn check(b: &[U256], rates: &[U256], amp: U256, t: &Template, g: &StableGuard) -> Option<bool> {
    if b.len() != 2 || rates.len() != 2 || g.oracle.len() != 2 || g.threshold.len() != 2 {
        return None;
    }
    let p = precision();
    let value = U256::from(100u8).checked_mul(p)?;
    // 10^dps = 10^36 / RATES (RATES = 1e18·10^(18 − dps), L136-L137); PRECISION_MUL = RATES / 1e18.
    let e36 = p.checked_mul(p)?;
    let (o0, o1) = (g.oracle[0], g.oracle[1]);
    let (m0, m1) = (rates[0] / p, rates[1] / p);
    if o0.is_zero() || o1.is_zero() || rates[0].is_zero() || rates[1].is_zero() {
        return None;
    }
    let dx0 = value.checked_mul(e36 / rates[0])? / o0;
    let dx1 = value.checked_mul(e36 / rates[1])? / o1;
    let dy1 = dy_no_fee(0, 1, dx0, b, rates, amp, t)?;
    let dy0 = dy_no_fee(1, 0, dx1, b, rates, amp, t)?;
    let den0 = dy0.checked_mul(m0)?;
    let den1 = dy1.checked_mul(m1)?;
    if den0.is_zero() || den1.is_zero() {
        return None;
    }
    let price0 = dx1.checked_mul(m1)?.checked_mul(o1)? / den0;
    let price1 = dx0.checked_mul(m0)?.checked_mul(o0)? / den1;
    let diff = |a: U256, b: U256| if a > b { a - b } else { b - a };
    let ok0 = diff(price0, o0).checked_mul(p)? <= o0.checked_mul(g.threshold[0])?;
    let ok1 = diff(price1, o1).checked_mul(p)? <= o1.checked_mul(g.threshold[1])?;
    Some(ok0 && ok1)
}
