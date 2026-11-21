//! Файл: venues/bsc_dex/math/gyro_2clp.rs
//! Что делает: решение 452 — порт Gyro 2-CLP (Balancer V3): инвариант пула квадратным уравнением (calculateInvariant с
//! округлением вниз / вверх), виртуальные смещения (√α, √β), выход на точный вход и вход на точный выход; корень — Ньютон
//! GyroPoolMath.sqrt. Балансы и суммы — живые 18 знаков, два токена (индексы 0 и 1).
//! Кто вызывает: math/balancer_v3_pool.rs.
//! От чего зависит: math/balancer_math.rs (FixedPoint V3 mulDown / mulUp / divDown / divUp — те же, что в источнике),
//! math/gyro_sqrt.rs (GyroPoolMath.sqrt).
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20260123-v3-gyro-2clp-v2 (=
//! v3/deprecated/20250120-v3-gyro-2clp: Gyro2CLPPool.sol, Gyro2CLPMath.sol, GyroPoolMath.sol побайтно равны) →
//! contracts/Gyro2CLPPool.sol onSwap L118-L152, _getVirtualOffsets L165-L199 (инвариант ROUND_DOWN; смещение входа —
//! вверх, выхода — вниз); contracts/lib/Gyro2CLPMath.sol calculateInvariant L24-L49, calculateQuadraticTerms L65-L103,
//! calculateQuadratic L115-L139 (корень с допуском 5), calcOutGivenIn L157-L190, calcInGivenOut L196-L227 (выход >
//! баланса — откат AssetBoundsExceeded), calculateVirtualParameter0 / 1 L230-L245. Solidity 0.8 — проверяемая
//! арифметика: переполнение, недобор, деление на ноль — None.

use alloy_primitives::U256;

use super::balancer_math::{div_down, div_up, mul_down, mul_up, one};
use super::gyro_sqrt::sqrt;

/// Пара «вниз / вверх» по округлению (Rounding.ROUND_DOWN — down = true).
fn div_r(a: U256, b: U256, down: bool) -> Option<U256> {
    if down { div_down(a, b) } else { div_up(a, b) }
}

/// mulDown при ROUND_DOWN, mulUp при ROUND_UP (_mulUpOrDown).
fn mul_r(a: U256, b: U256, down: bool) -> Option<U256> {
    if down { mul_down(a, b) } else { mul_up(a, b) }
}

/// Инвариант (calculateInvariant L24-L49, calculateQuadraticTerms L65-L103, calculateQuadratic L115-L139). Принимает:
/// балансы токенов 0 и 1, √α, √β, округление вниз ли. None — откат.
pub fn invariant(b0: U256, b1: U256, sa: U256, sb: U256, down: bool) -> Option<U256> {
    let two = U256::from(2u8);
    let a = one().checked_sub(div_r(sa, sb, down)?)?;
    let mb = div_r(b1, sb, down)?.checked_add(mul_r(b0, sa, down)?)?;
    let mc = mul_r(b0, b1, down)?;
    let sq1 = mul_r(mul_r(mul_r(b0, b0, down)?, sa, down)?, sa, down)?;
    let sq2 = div_r(
        two.checked_mul(mul_r(mul_r(b0, b1, down)?, sa, down)?)?,
        sb,
        down,
    )?;
    // _mulDownOrUp: при ROUND_DOWN — mulUp (знаменатель вверх), при ROUND_UP — mulDown.
    let sq3 = div_r(mul_r(b1, b1, down)?, mul_r(sb, sb, !down)?, down)?;
    let b_sq = sq1.checked_add(sq2)?.checked_add(sq3)?;
    let denominator = two.checked_mul(a)?;
    let add_term = mul_r(U256::from(4u8).checked_mul(mc)?, a, down)?;
    let root = sqrt(b_sq.checked_add(add_term)?, U256::from(5u8))?;
    div_r(mb.checked_add(root)?, denominator, down)
}

/// onSwap пула (Gyro2CLPPool.sol L118-L152). Принимает: живые балансы (2), индексы входа и выхода, сумму, точный ли
/// вход, √α, √β. Возвращает: выход или вход (18 знаков). None — откат.
pub fn on_swap(
    bal: &[U256],
    i: usize,
    j: usize,
    a: U256,
    exact_in: bool,
    sa: U256,
    sb: U256,
) -> Option<U256> {
    if bal.len() != 2 || i > 1 || j > 1 || i == j {
        return None;
    }
    let (b_in, b_out) = (bal[i], bal[j]);
    // _getVirtualOffsets: инвариант по балансам в порядке пула, ROUND_DOWN.
    let inv = invariant(bal[0], bal[1], sa, sb, true)?;
    let (v_in, v_out) = if i == 0 {
        (div_up(inv, sb)?, mul_down(inv, sa)?)
    } else {
        (mul_up(inv, sa)?, div_down(inv, sb)?)
    };
    let virt_in = b_in.checked_add(v_in)?;
    let virt_out = b_out.checked_add(v_out)?;
    if exact_in {
        // calcOutGivenIn L179-L189.
        let out = div_down(mul_down(virt_out, a)?, virt_in.checked_add(a)?)?;
        (out <= b_out).then_some(out)
    } else {
        // calcInGivenOut L217-L226.
        if a > b_out {
            return None;
        }
        div_up(mul_up(virt_in, a)?, virt_out.checked_sub(a)?)
    }
}
