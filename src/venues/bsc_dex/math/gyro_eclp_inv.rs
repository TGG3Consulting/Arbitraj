//! Файл: venues/bsc_dex/math/gyro_eclp_inv.rs
//! Что делает: решение 452 — инвариант пула Gyro E-CLP с оценкой ошибки (calculateInvariantWithError), его части и
//! граница нового баланса (checkAssetBounds):
//! At·Aχ, (Aχ)·(Aχ) в расширенной точности, три слагаемых подкоренного выражения. Порт дословный, с тем же порядком
//! округлений; арифметика — math/gyro_signed.rs.
//! Кто вызывает: math/gyro_eclp.rs.
//! От чего зависит: core/types (EclpParams), math/gyro_signed.rs, math/gyro_sqrt.rs.
//! Настройки: нет.
//!
//! Источник (решение 23): balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20260126-v3-gyro-eclp-v2 →
//! contracts/lib/GyroECLPMath.sol: _ONE_XP L51, _MAX_BALANCES L61, _MAX_INVARIANT L62, calculateInvariantWithError
//! L333-L384, calcAtAChi L395-L416, calcAChiAChiInXp L428-L449, calcMinAtxAChiySqPlusAtxSq L459-L488,
//! calc2AtxAtyAChixAChiy L499-L518, calcMinAtyAChixSqPlusAtySq L528-L551, calcInvariantSqrt L565-L583;
//! maxBalances0 L279-L293, maxBalances1 L306-L317, checkAssetBounds L743-L757 (граница нового баланса). Те же функции у
//! v3/deprecated/20250124-v3-gyro-eclp (diff без комментариев: отличия только в validateParams, mulAinv, computePrice и
//! в том, что calcGiven возвращает ещё смещения).

use alloy_primitives::I256;

use crate::core::types::pool_vault::EclpParams;

use super::gyro_signed::{
    add, div, div_down_mag, div_up_mag, div_xp, int, mul, mul_down_mag, mul_down_xp_to_np,
    mul_up_mag, mul_up_xp_to_np, mul_xp, neg, one_xp, sub, ten, to_i, to_u,
};
use super::gyro_sqrt::sqrt;

/// Инвариант и его ошибка (calculateInvariantWithError L333-L384). Принимает: балансы x, y (18 знаков), параметры.
/// Возвращает: (инвариант, ошибка). None — x + y > _MAX_BALANCES, инвариант + ошибка > _MAX_INVARIANT, откат арифметики.
pub fn with_error(x: I256, y: I256, p: &EclpParams) -> Option<(I256, I256)> {
    if add(x, y)? > ten(34) {
        return None;
    }
    let at_a_chi = at_a_chi(x, y, p)?;
    let (root, mut err) = inv_sqrt(x, y, p)?;
    if root.is_positive() {
        err = div_up_mag(add(err, I256::ONE)?, mul(int(2), root)?)?;
    } else {
        err = if err.is_positive() {
            to_i(sqrt(to_u(err)?, alloy_primitives::U256::from(5u8))?)?
        } else {
            ten(9)
        };
    }
    let lam_xy = div(mul_up_mag(p.lambda, add(x, y)?)?, one_xp())?;
    err = mul(add(add(lam_xy, err)?, I256::ONE)?, int(20))?;
    let mul_den = div_xp(one_xp(), sub(a_chi_a_chi_xp(p)?, one_xp())?)?;
    let inv = mul_down_xp_to_np(sub(add(at_a_chi, root)?, err)?, mul_den)?;
    err = mul_up_xp_to_np(err, mul_den)?;
    let lam2 = div(mul(p.lambda, p.lambda)?, ten(36))?;
    let term = div(
        mul(mul(mul_up_xp_to_np(inv, mul_den)?, lam2)?, int(40))?,
        one_xp(),
    )?;
    err = add(add(err, term)?, I256::ONE)?;
    if add(inv, err)? > mul(int(3), ten(37))? {
        return None;
    }
    Some((inv, err))
}

/// calcAtAChi (L395-L416).
fn at_a_chi(x: I256, y: I256, p: &EclpParams) -> Option<I256> {
    let d_sq2 = mul_xp(p.d_sq, p.d_sq)?;
    let term_xp = div_xp(
        div_down_mag(add(div_down_mag(p.w, p.lambda)?, p.z)?, p.lambda)?,
        d_sq2,
    )?;
    let cx_sy = sub(mul_down_mag(x, p.c)?, mul_down_mag(y, p.s)?)?;
    let mut val = mul_down_xp_to_np(cx_sy, term_xp)?;
    let lam_x = mul_down_mag(mul_down_mag(x, p.lambda)?, p.s)?;
    let term_np = add(lam_x, mul_down_mag(mul_down_mag(y, p.lambda)?, p.c)?)?;
    val = add(val, mul_down_xp_to_np(term_np, div_xp(p.u, d_sq2)?)?)?;
    let term_np = add(mul_down_mag(x, p.s)?, mul_down_mag(y, p.c)?)?;
    add(val, mul_down_xp_to_np(term_np, div_xp(p.v, d_sq2)?)?)
}

/// calcAChiAChiInXp (L428-L449).
pub fn a_chi_a_chi_xp(p: &EclpParams) -> Option<I256> {
    let d_sq3 = mul_xp(mul_xp(p.d_sq, p.d_sq)?, p.d_sq)?;
    let mut val = mul_up_mag(p.lambda, div_xp(mul_xp(mul(int(2), p.u)?, p.v)?, d_sq3)?)?;
    let u1 = add(p.u, I256::ONE)?;
    let uu = div_xp(mul_xp(u1, u1)?, d_sq3)?;
    val = add(val, mul_up_mag(mul_up_mag(uu, p.lambda)?, p.lambda)?)?;
    val = add(val, div_xp(mul_xp(p.v, p.v)?, d_sq3)?)?;
    let term_xp = add(div_up_mag(p.w, p.lambda)?, p.z)?;
    add(val, div_xp(mul_xp(term_xp, term_xp)?, d_sq3)?)
}

/// dSq⁴ (L478, L515, L545).
fn d_sq4(p: &EclpParams) -> Option<I256> {
    mul_xp(mul_xp(mul_xp(p.d_sq, p.d_sq)?, p.d_sq)?, p.d_sq)
}

/// calcMinAtxAChiySqPlusAtxSq (L459-L488).
fn min_atx(x: I256, y: I256, p: &EclpParams) -> Option<I256> {
    let xx = mul_up_mag(mul_up_mag(mul_up_mag(x, x)?, p.c)?, p.c)?;
    let yy = mul_up_mag(mul_up_mag(mul_up_mag(y, y)?, p.s)?, p.s)?;
    let xy = mul_down_mag(mul_down_mag(mul_down_mag(x, y)?, mul(p.c, int(2))?)?, p.s)?;
    let term_np = sub(add(xx, yy)?, xy)?;
    let uu = mul_xp(p.u, p.u)?;
    let uv = div_down_mag(mul_xp(mul(int(2), p.u)?, p.v)?, p.lambda)?;
    let vv = div_down_mag(div_down_mag(mul_xp(p.v, p.v)?, p.lambda)?, p.lambda)?;
    let term_xp = div_xp(add(add(uu, uv)?, vv)?, d_sq4(p)?)?;
    let val = mul_down_xp_to_np(neg(term_np)?, term_xp)?;
    let at = div_down_mag(div_down_mag(sub(term_np, int(9))?, p.lambda)?, p.lambda)?;
    add(val, mul_down_xp_to_np(at, div_xp(one_xp(), p.d_sq)?)?)
}

/// calc2AtxAtyAChixAChiy (L499-L518).
fn two_at_a_chi(x: I256, y: I256, p: &EclpParams) -> Option<I256> {
    let x2_y2 = sub(mul_down_mag(x, x)?, mul_up_mag(y, y)?)?;
    let mut term_np = mul_down_mag(mul_down_mag(x2_y2, mul(int(2), p.c)?)?, p.s)?;
    let xy = mul_down_mag(y, mul(int(2), x)?)?;
    let cc = mul_down_mag(mul_down_mag(xy, p.c)?, p.c)?;
    let ss = mul_down_mag(mul_down_mag(xy, p.s)?, p.s)?;
    term_np = sub(add(term_np, cc)?, ss)?;
    let wv = div_down_mag(div_down_mag(mul_xp(p.w, p.v)?, p.lambda)?, p.lambda)?;
    let mut term_xp = add(mul_xp(p.z, p.u)?, wv)?;
    let wu_zv = add(mul_xp(p.w, p.u)?, mul_xp(p.z, p.v)?)?;
    term_xp = add(term_xp, div_down_mag(wu_zv, p.lambda)?)?;
    term_xp = div_xp(term_xp, d_sq4(p)?)?;
    mul_down_xp_to_np(term_np, term_xp)
}

/// calcMinAtyAChixSqPlusAtySq (L528-L551).
fn min_aty(x: I256, y: I256, p: &EclpParams) -> Option<I256> {
    let xx = mul_up_mag(mul_up_mag(mul_up_mag(x, x)?, p.s)?, p.s)?;
    let yy = mul_up_mag(mul_up_mag(mul_up_mag(y, y)?, p.c)?, p.c)?;
    let xy = mul_up_mag(mul_up_mag(mul_up_mag(x, y)?, mul(p.s, int(2))?)?, p.c)?;
    let term_np = add(add(xx, yy)?, xy)?;
    let zz = mul_xp(p.z, p.z)?;
    let ww = div_down_mag(div_down_mag(mul_xp(p.w, p.w)?, p.lambda)?, p.lambda)?;
    let mut term_xp = add(zz, ww)?;
    term_xp = add(
        term_xp,
        div_down_mag(mul_xp(mul(int(2), p.z)?, p.w)?, p.lambda)?,
    )?;
    term_xp = div_xp(term_xp, d_sq4(p)?)?;
    let val = mul_down_xp_to_np(neg(term_np)?, term_xp)?;
    add(
        val,
        mul_down_xp_to_np(sub(term_np, int(9))?, div_xp(one_xp(), p.d_sq)?)?,
    )
}

/// calcInvariantSqrt (L565-L583): (корень, ошибка). Подкоренное ≤ 0 — корень 0.
fn inv_sqrt(x: I256, y: I256, p: &EclpParams) -> Option<(I256, I256)> {
    let val = add(
        add(min_atx(x, y, p)?, two_at_a_chi(x, y, p)?)?,
        min_aty(x, y, p)?,
    )?;
    let err = div(add(mul_up_mag(x, x)?, mul_up_mag(y, y)?)?, ten(38))?;
    let root = if val.is_positive() {
        to_i(sqrt(to_u(val)?, alloy_primitives::U256::from(5u8))?)?
    } else {
        I256::ZERO
    };
    Some((root, err))
}

/// maxBalances0 / maxBalances1 (L279-L317). Принимает: параметры, r, индекс токена. Возвращает: наибольший баланс.
fn max_balance(p: &EclpParams, rx: I256, ry: I256, idx: usize) -> Option<I256> {
    let t1 = div_xp(sub(p.tau_beta_x, p.tau_alpha_x)?, p.d_sq)?;
    let (first, second) = if idx == 0 { (p.c, p.s) } else { (p.s, p.c) };
    let dy = if idx == 0 {
        sub(p.tau_beta_y, p.tau_alpha_y)?
    } else {
        sub(p.tau_alpha_y, p.tau_beta_y)?
    };
    let t2 = div_xp(dy, p.d_sq)?;
    let v = mul_down_xp_to_np(mul_down_mag(mul_down_mag(ry, p.lambda)?, first)?, t1)?;
    let w = if t2.is_positive() {
        mul_down_mag(ry, second)?
    } else {
        mul_up_mag(rx, second)?
    };
    add(v, mul_down_xp_to_np(w, t2)?)
}

/// checkAssetBounds (L743-L757): новый баланс ≤ _MAX_BALANCES и ≤ наибольшего. Ошибок нет кроме переполнения.
pub fn in_bounds(p: &EclpParams, rx: I256, ry: I256, bal: I256, idx: usize) -> Option<bool> {
    Some(bal <= ten(34) && bal <= max_balance(p, rx, ry, idx)?)
}
