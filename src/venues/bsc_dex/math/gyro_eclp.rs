//! Файл: venues/bsc_dex/math/gyro_eclp.rs
//! Что делает: решение 452 — onSwap пула Gyro E-CLP (Balancer V3): инвариант с ошибкой (math/gyro_eclp_inv.rs) →
//! вектор r = (инвариант + 2·ошибка, инвариант), виртуальные смещения, проверка границ баланса входа, новый баланс
//! выхода решением квадратного уравнения (solveQuadraticSwap, calcXpXpDivLambdaLambda). Точный вход и точный выход.
//! Кто вызывает: math/balancer_v3_pool.rs.
//! От чего зависит: core/types (EclpParams), math/gyro_signed.rs, math/gyro_eclp_inv.rs, math/gyro_sqrt.rs.
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20260126-v3-gyro-eclp-v2 →
//! contracts/GyroECLPPool.sol onSwap L164-L204 (invariant = Vector2(inv + 2·err, inv)); contracts/lib/GyroECLPMath.sol
//! virtualOffset0 L227-L241, virtualOffset1 L252-L266, maxBalances0 L279-L293, maxBalances1 L306-L317,
//! checkAssetBounds L743-L757, calcOutGivenIn L771-L804, calcInGivenOut L819-L853, solveQuadraticSwap L872-L928,
//! calcXpXpDivLambdaLambda L944-L1008, calcYGivenX L1023-L1036, calcXGivenY L1051-L1074. У
//! v3/deprecated/20250124-v3-gyro-eclp те же формулы обмена (сверено diff без комментариев).

use alloy_primitives::{I256, U256};

use crate::core::types::pool_vault::EclpParams;

use super::gyro_eclp_inv::{in_bounds, with_error};
use super::gyro_signed::{
    add, div_down_mag, div_up_mag, div_xp, int, mul, mul_down_mag, mul_down_xp_to_np, mul_up_mag,
    mul_up_xp_to_np, mul_xp, neg, one_xp, sub, to_i, to_u,
};
use super::gyro_sqrt::sqrt;

/// virtualOffset0 (L227-L241). r = (rx, ry).
fn offset0(p: &EclpParams, rx: I256, ry: I256) -> Option<I256> {
    let term = div_xp(p.tau_beta_x, p.d_sq)?;
    let a = if p.tau_beta_x.is_positive() {
        mul_up_xp_to_np(mul_up_mag(mul_up_mag(rx, p.lambda)?, p.c)?, term)?
    } else {
        mul_up_xp_to_np(mul_down_mag(mul_down_mag(ry, p.lambda)?, p.c)?, term)?
    };
    let t2 = div_xp(p.tau_beta_y, p.d_sq)?;
    add(a, mul_up_xp_to_np(mul_up_mag(rx, p.s)?, t2)?)
}

/// virtualOffset1 (L252-L266).
fn offset1(p: &EclpParams, rx: I256, ry: I256) -> Option<I256> {
    let term = div_xp(p.tau_alpha_x, p.d_sq)?;
    let b = if p.tau_alpha_x.is_negative() {
        mul_up_xp_to_np(mul_up_mag(mul_up_mag(rx, p.lambda)?, p.s)?, neg(term)?)?
    } else {
        mul_up_xp_to_np(mul_down_mag(mul_down_mag(neg(ry)?, p.lambda)?, p.s)?, term)?
    };
    let t2 = div_xp(p.tau_alpha_y, p.d_sq)?;
    add(b, mul_up_xp_to_np(mul_up_mag(rx, p.c)?, t2)?)
}

/// calcXpXpDivLambdaLambda (L944-L1008). tau = (tx, ty) — tauBeta (calcYGivenX) или (−tauAlpha.x, tauAlpha.y).
#[allow(clippy::too_many_arguments)]
fn xp_xp(
    x: I256,
    rx: I256,
    ry: I256,
    lam: I256,
    s: I256,
    c: I256,
    tx: I256,
    ty: I256,
    d_sq: I256,
) -> Option<I256> {
    let sq_x = mul_xp(d_sq, d_sq)?;
    let sq_y = mul_up_mag(rx, rx)?;
    let seven = int(7);
    let term = div_xp(mul_xp(tx, ty)?, sq_x)?;
    let mut qa = if term.is_positive() {
        let q = mul_up_mag(sq_y, mul(int(2), s)?)?;
        mul_up_xp_to_np(mul_up_mag(q, c)?, add(term, seven)?)?
    } else {
        let q = mul_down_mag(mul_down_mag(ry, ry)?, mul(int(2), s)?)?;
        mul_up_xp_to_np(mul_down_mag(q, c)?, term)?
    };
    let qb = if tx.is_negative() {
        let k = add(neg(div_xp(tx, d_sq)?)?, int(3))?;
        mul_up_xp_to_np(mul_up_mag(mul_up_mag(rx, x)?, mul(int(2), c)?)?, k)?
    } else {
        let v = mul_down_mag(mul_down_mag(neg(ry)?, x)?, mul(int(2), c)?)?;
        mul_up_xp_to_np(v, div_xp(tx, d_sq)?)?
    };
    qa = add(qa, qb)?;
    let term = add(div_xp(mul_xp(ty, ty)?, sq_x)?, seven)?;
    let mut qb = mul_up_xp_to_np(mul_up_mag(mul_up_mag(sq_y, s)?, s)?, term)?;
    let v = mul_down_mag(mul_down_mag(neg(ry)?, x)?, mul(int(2), s)?)?;
    let qc = mul_up_xp_to_np(v, div_xp(ty, d_sq)?)?;
    qb = add(add(qb, qc)?, mul_up_mag(x, x)?)?;
    qb = if qb.is_positive() {
        div_up_mag(qb, lam)?
    } else {
        div_down_mag(qb, lam)?
    };
    qa = add(qa, qb)?;
    qa = if qa.is_positive() {
        div_up_mag(qa, lam)?
    } else {
        div_down_mag(qa, lam)?
    };
    let term = add(div_xp(mul_xp(tx, tx)?, sq_x)?, seven)?;
    let val = mul_up_mag(mul_up_mag(sq_y, c)?, c)?;
    add(mul_up_xp_to_np(val, term)?, qa)
}

/// solveQuadraticSwap (L872-L928). ab — смещения (свой порядок у calcXGivenY), tau — как у xp_xp.
fn solve(
    p: &EclpParams,
    x: I256,
    s: I256,
    c: I256,
    r: (I256, I256),
    ab: (I256, I256),
    tau: (I256, I256),
) -> Option<I256> {
    let (lam, d_sq) = (p.lambda, p.d_sq);
    let lam_x = sub(one_xp(), div_down_mag(div_down_mag(one_xp(), lam)?, lam)?)?;
    let lam_y = sub(one_xp(), div_up_mag(div_up_mag(one_xp(), lam)?, lam)?)?;
    let xp = sub(x, ab.0)?;
    let qb = if xp.is_positive() {
        let v = mul_down_mag(mul_down_mag(neg(xp)?, s)?, c)?;
        mul_up_xp_to_np(v, div_xp(lam_y, d_sq)?)?
    } else {
        let v = mul_up_mag(mul_up_mag(neg(xp)?, s)?, c)?;
        mul_up_xp_to_np(v, add(div_xp(lam_x, d_sq)?, I256::ONE)?)?
    };
    let st_x = div_xp(mul_down_mag(mul_down_mag(lam_y, s)?, s)?, d_sq)?;
    let st_y = div_xp(mul_up_mag(mul_up_mag(lam_x, s)?, s)?, add(d_sq, I256::ONE)?)?;
    let st_y = add(st_y, I256::ONE)?;
    let (st_x, st_y) = (sub(one_xp(), st_x)?, sub(one_xp(), st_y)?);
    let mut qc = neg(xp_xp(x, r.0, r.1, lam, s, c, tau.0, tau.1, d_sq)?)?;
    qc = add(qc, mul_down_xp_to_np(mul_down_mag(r.1, r.1)?, st_y)?)?;
    qc = if qc.is_positive() {
        to_i(sqrt(to_u(qc)?, U256::from(5u8))?)?
    } else {
        I256::ZERO
    };
    let diff = sub(qb, qc)?;
    let qa = if diff.is_positive() {
        mul_up_xp_to_np(diff, add(div_xp(one_xp(), st_y)?, I256::ONE)?)?
    } else {
        mul_up_xp_to_np(diff, div_xp(one_xp(), st_x)?)?
    };
    add(qa, ab.1)
}

/// onSwap пула (GyroECLPPool.sol L164-L204 + calcOutGivenIn / calcInGivenOut). Принимает: параметры, живые балансы (2),
/// индексы, сумму, точный ли вход. Возвращает: выход или вход (18 знаков). None — откат.
pub fn on_swap(
    p: &EclpParams,
    bal: &[U256],
    i: usize,
    j: usize,
    a: U256,
    exact_in: bool,
) -> Option<U256> {
    if bal.len() != 2 || i > 1 || j > 1 || i == j {
        return None;
    }
    let (inv, err) = with_error(to_i(bal[0])?, to_i(bal[1])?, p)?;
    let (rx, ry) = (add(inv, mul(int(2), err)?)?, inv);
    let (oa, ob) = (offset0(p, rx, ry)?, offset1(p, rx, ry)?);
    // calcYGivenX: дан баланс токена 0; calcXGivenY: дан баланс токена 1 (s ↔ c, смещения и tau — свои).
    let given_y = |given: I256, of: usize| -> Option<I256> {
        if of == 0 {
            solve(
                p,
                given,
                p.s,
                p.c,
                (rx, ry),
                (oa, ob),
                (p.tau_beta_x, p.tau_beta_y),
            )
        } else {
            let tau = (neg(p.tau_alpha_x)?, p.tau_alpha_y);
            solve(p, given, p.c, p.s, (rx, ry), (ob, oa), tau)
        }
    };
    if exact_in {
        let new_in = to_i(bal[i].checked_add(a)?)?;
        if !in_bounds(p, rx, ry, new_in, i)? {
            return None;
        }
        let new_out = to_u(given_y(new_in, i)?)?;
        bal[j].checked_sub(new_out)
    } else {
        if a > bal[j] {
            return None;
        }
        let new_out = to_i(bal[j] - a)?;
        let new_in = given_y(new_out, j)?;
        if !in_bounds(p, rx, ry, new_in, i)? {
            return None;
        }
        to_u(new_in)?.checked_sub(bal[i])
    }
}
