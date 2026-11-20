//! Файл: venues/bsc_dex/math/balancer_v3_stable.rs
//! Что делает: решение 441 (T433-F2) — порт StableMath Balancer V3: инвариант D итерацией (computeInvariant), баланс
//! токена по D и остальным балансам (computeBalance), выход на точный вход и вход на точный выход. Балансы и суммы —
//! живые 18 знаков (множитель и курс применило хранилище), комиссия снята снаружи (math/balancer_v3.rs).
//! Кто вызывает: math/balancer_v3.rs.
//! От чего зависит: alloy-primitives.
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8, build-info v3/tasks/20260116-v3-stable-pool-v3 →
//! @balancer-labs/v3-solidity-utils/contracts/math/StableMath.sol: AMP_PRECISION = 1e3 (L38), MAX_IMBALANCE_RATIO =
//! 10 000 (L41), computeInvariant L68-L119 (255 раундов, несходимость — откат L118), computeOutGivenExactIn L132-L165
//! (выход − 1), computeInGivenExactOut L178-L211 (вход + 1), computeBalance L222-L268 (255 раундов, откат L267),
//! getMinAndMaxBalances L276-L293, ensureBalancesWithinMaxImbalanceRange L300-L305. У выпусков 20241205-v3-stable-pool и
//! 20250324-v3-stable-pool-v2 те же функции побайтно по смыслу (diff: только MAX_AMP и комментарии; проверки дисбаланса
//! у них нет). Solidity 0.8 — арифметика с откатом при переполнении и делении на ноль: здесь None.
//! divUpRaw — FixedPoint.sol L79-L90 (b = 0 — откат; a = 0 → 0, иначе 1 + (a − 1) / b).

use alloy_primitives::U256;

/// AMP_PRECISION (StableMath.sol L38).
const AMP_PRECISION: u64 = 1_000;
/// Предел раундов итерации (L94, L249).
const ROUNDS: usize = 255;
/// MAX_IMBALANCE_RATIO (L41).
const MAX_IMBALANCE_RATIO: u64 = 10_000;

/// divUpRaw (FixedPoint.sol L79-L90). None — деление на ноль.
fn div_up_raw(a: U256, b: U256) -> Option<U256> {
    if b.is_zero() {
        return None;
    }
    Some(if a.is_zero() {
        a
    } else {
        U256::from(1u8) + (a - U256::from(1u8)) / b
    })
}

/// Разница не больше 1 — условие схождения (L106-L115, L256-L264). Ошибок нет.
fn close(a: U256, b: U256) -> bool {
    if a > b {
        a - b <= U256::from(1u8)
    } else {
        b - a <= U256::from(1u8)
    }
}

/// computeInvariant (L68-L119). Принимает: A × точность, балансы (18 знаков). Возвращает: D; сумма 0 → 0. None —
/// несходимость за 255 раундов, деление на ноль (нулевой баланс), переполнение.
pub fn invariant(amp: U256, balances: &[U256]) -> Option<U256> {
    let n = U256::from(balances.len());
    let mut sum = U256::ZERO;
    for b in balances {
        sum = sum.checked_add(*b)?;
    }
    if sum.is_zero() {
        return Some(U256::ZERO);
    }
    let prec = U256::from(AMP_PRECISION);
    let mut inv = sum;
    let amp_total = amp.checked_mul(n)?;
    for _ in 0..ROUNDS {
        let mut d_p = inv;
        for b in balances {
            let den = b.checked_mul(n)?;
            if den.is_zero() {
                return None;
            }
            d_p = d_p.checked_mul(inv)? / den;
        }
        let prev = inv;
        let a_s = amp_total.checked_mul(sum)? / prec;
        let num = a_s.checked_add(d_p.checked_mul(n)?)?.checked_mul(inv)?;
        let a_d = amp_total.checked_sub(prec)?.checked_mul(inv)? / prec;
        let n1 = n.checked_add(U256::from(1u8))?;
        let den = a_d.checked_add(n1.checked_mul(d_p)?)?;
        if den.is_zero() {
            return None;
        }
        inv = num / den;
        if close(inv, prev) {
            return Some(inv);
        }
    }
    None
}

/// computeBalance (L222-L268). Принимает: A × точность, балансы, D, индекс токена. Возвращает: баланс токена. None —
/// несходимость, деление на ноль, переполнение или отрицательное (b + 2y − D < 0).
pub fn balance(amp: U256, balances: &[U256], inv: U256, index: usize) -> Option<U256> {
    let n = U256::from(balances.len());
    let amp_total = amp.checked_mul(n)?;
    let mut sum = *balances.first()?;
    let mut p_d = sum.checked_mul(n)?;
    for b in balances.iter().skip(1) {
        if inv.is_zero() {
            return None;
        }
        p_d = p_d.checked_mul(*b)?.checked_mul(n)? / inv;
        sum = sum.checked_add(*b)?;
    }
    sum = sum.checked_sub(*balances.get(index)?)?;
    let prec = U256::from(AMP_PRECISION);
    let inv2 = inv.checked_mul(inv)?;
    let c0 = div_up_raw(inv2.checked_mul(prec)?, amp_total.checked_mul(p_d)?)?;
    let c = c0.checked_mul(*balances.get(index)?)?;
    if amp_total.is_zero() {
        return None;
    }
    let b = sum.checked_add(inv.checked_mul(prec)? / amp_total)?;
    let mut y = div_up_raw(inv2.checked_add(c)?, inv.checked_add(b)?)?;
    for _ in 0..ROUNDS {
        let prev = y;
        let two_y = y.checked_mul(U256::from(2u8))?;
        let den = two_y.checked_add(b)?.checked_sub(inv)?;
        y = div_up_raw(y.checked_mul(y)?.checked_add(c)?, den)?;
        if close(y, prev) {
            return Some(y);
        }
    }
    None
}

/// computeOutGivenExactIn (L132-L165): баланс входа + вход, новый баланс выхода по D, выход = баланс − новый − 1.
/// Принимает: A, балансы, индексы, вход (18 знаков, без комиссии), D. None — откат формулы.
pub fn out_given_in(
    amp: U256,
    balances: &[U256],
    i: usize,
    j: usize,
    a_in: U256,
    inv: U256,
) -> Option<U256> {
    let mut b = balances.to_vec();
    *b.get_mut(i)? = b.get(i)?.checked_add(a_in)?;
    let fin = balance(amp, &b, inv, j)?;
    let left = balances.get(j)?.checked_sub(fin)?;
    left.checked_sub(U256::from(1u8))
}

/// computeInGivenExactOut (L178-L211): баланс выхода − выход, новый баланс входа по D, вход = новый − баланс + 1.
/// Принимает: A, балансы, индексы, выход (18 знаков), D. None — откат формулы.
pub fn in_given_out(
    amp: U256,
    balances: &[U256],
    i: usize,
    j: usize,
    a_out: U256,
    inv: U256,
) -> Option<U256> {
    let mut b = balances.to_vec();
    *b.get_mut(j)? = b.get(j)?.checked_sub(a_out)?;
    let fin = balance(amp, &b, inv, i)?;
    let grow = fin.checked_sub(*balances.get(i)?)?;
    grow.checked_add(U256::from(1u8))
}

/// Проверка дисбаланса выпуска 20260116-v3-stable-pool-v3 (StablePool.sol L195-L247 + StableMath.sol L276-L305): min и
/// max балансов до обмена, затем max(…, новый баланс входа), min(…, новый баланс выхода); max / min (целое) < 10 000.
/// Принимает: балансы, индексы, вход и выход (18 знаков). Возвращает: true — обмен пройдёт. Ошибок нет (min = 0 —
/// деление на ноль в контракте, откат: false; отрицательный баланс выхода — false).
pub fn within_imbalance(balances: &[U256], i: usize, j: usize, a_in: U256, a_out: U256) -> bool {
    let (Some(min0), Some(max0)) = (balances.iter().min(), balances.iter().max()) else {
        return false;
    };
    let (Some(bi), Some(bo)) = (balances.get(i), balances.get(j)) else {
        return false;
    };
    let (Some(new_in), Some(new_out)) = (bi.checked_add(a_in), bo.checked_sub(a_out)) else {
        return false;
    };
    let max = (*max0).max(new_in);
    let min = (*min0).min(new_out);
    !min.is_zero() && max / min < U256::from(MAX_IMBALANCE_RATIO)
}
