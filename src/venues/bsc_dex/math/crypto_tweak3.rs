//! Файл: venues/bsc_dex/math/crypto_tweak3.rs
//! Что делает: решение 472 — модель откатов `tweak_price` пула Tricrypto-NG после обмена (как math/crypto_tweak2.rs у
//! Twocrypto-NG; шаги и оговорка о времени блока — там же). Отличия трёх монет: get_p даёт две цены (crypto_p3.rs),
//! virtual_price — через geometric_mean, норма отклонения оракула — isqrt суммы квадратов, цены упаковываются по
//! 128 бит (`_pack_prices`: assert p < 2^128 − 1 — новые last_prices, оракул и p_new), доли x/D после подстройки
//! проверяются у обеих реализаций (WETH и без нативного ETH — тот же итог).
//! Кто вызывает: math/crypto_swap.rs (exchange_out, Tricrypto-NG при CryptoData.tweak).
//! От чего зависит: core/types (CryptoData, CryptoTweak), math/crypto_d3.rs, crypto_p3.rs, crypto_ops.rs, math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/tricrypto-ng ecaa816, contracts/main/CurveTricryptoOptimizedWETH.vy — вызов L956
//! (`tweak_price(A_gamma, xp, 0, y_out[1])`), `tweak_price` L964-L1163: оракул L1005-L1034 (упаковка L1033), newton_D
//! L1044, get_p и упаковка last_prices L1048-L1051, xp' L1055-L1058, virtual_price / assert «Loss» L1066-L1079,
//! подстройка L1084-L1158 (норма L1094-L1104, p_new L1119-L1124, newton_D L1132, доли L1134-L1136, новый
//! virtual_price L1145-L1147, упаковка p_new L1155); `_pack_prices` L824-L837 (PRICE_MASK L208). CurveTricryptoOptimized.vy
//! того же коммита — tweak_price L885-L1096 (доли — `assert D > 0` и unsafe_div: тот же итог).

use alloy_primitives::U256;

use crate::core::types::{CryptoData, CryptoTweak};

use super::crypto_ops::{au, du, isqrt, mu, su, udiv_u};
use super::ten::{self, E18};
use super::{crypto_d3, crypto_p3};

/// PRICE_MASK = 2**128 − 1 (L208): `_pack_prices` требует p < PRICE_MASK (L835).
fn packable(p: U256) -> bool {
    p < (U256::from(1u8) << 128usize) - U256::from(1u8)
}

/// Пройдёт ли tweak_price после обмена. Принимает: состояние пула, состояние tweak_price, xp после обмена, K0_prev
/// (второе значение get_y). Возвращает: Some(()) — не откатится; None — откатится. Ошибок нет.
pub fn passes(p: &CryptoData, t: &CryptoTweak, xp: [U256; 3], k0_prev: U256) -> Option<()> {
    let (two, three) = (U256::from(2u8), U256::from(3u8));
    let ps = [*p.price_scale.first()?, *p.price_scale.get(1)?];
    let po = [*t.price_oracle.first()?, *t.price_oracle.get(1)?];
    if !po.iter().all(|v| packable(*v)) {
        return None;
    }
    let d = crypto_d3::newton_d_k0(p.a, p.gamma, xp, k0_prev)?;
    let lp = crypto_p3::get_p(xp, d, p.a, p.gamma)?;
    for (l, s) in lp.iter().zip(ps) {
        if !packable(udiv_u(mu(*l, s)?, E18)) {
            return None;
        }
    }
    let mut x = [udiv_u(d, three), U256::ZERO, U256::ZERO];
    for (xk, s) in x.iter_mut().skip(1).zip(ps) {
        *xk = du(mu(d, E18)?, mu(three, s)?)?;
    }
    let (mut vp, mut xcp_profit) = (E18, E18);
    if t.virtual_price > U256::ZERO {
        let xcp = crypto_d3::geometric_mean(x)?;
        vp = du(mu(E18, xcp)?, t.total_supply)?;
        xcp_profit = udiv_u(mu(t.xcp_profit, vp)?, t.virtual_price);
        if !p.ramp && vp <= t.virtual_price {
            return None;
        }
    }
    let lhs = su(mu(vp, two)?, E18)?;
    let rhs = au(xcp_profit, mu(two, t.allowed_extra_profit)?)?;
    if lhs <= rhs {
        return Some(());
    }
    rebalance(p, t, xp, ps, po, xcp_profit)
}

/// Подстройка price_scale (L1084-L1158). Принимает: состояние, tweak, xp после обмена, price_scale, price_oracle,
/// xcp_profit. Возвращает: Some(()) — не откатится; None — откатится. Ошибок нет.
fn rebalance(
    p: &CryptoData,
    t: &CryptoTweak,
    xp: [U256; 3],
    ps: [U256; 2],
    po: [U256; 2],
    xcp_profit: U256,
) -> Option<()> {
    let three = U256::from(3u8);
    let mut norm = U256::ZERO;
    for (o, s) in po.iter().zip(ps) {
        // ratio = unsafe_div(price_oracle[k] * 10**18, price_scale[k]) — умножение проверяемое; ratio**2 — тоже.
        let r = udiv_u(mu(*o, E18)?, s);
        let r = if r > E18 {
            r.wrapping_sub(E18)
        } else {
            E18.wrapping_sub(r)
        };
        norm = norm.wrapping_add(mu(r, r)?);
    }
    let norm = isqrt(norm);
    let step = t.adjustment_step.max(udiv_u(norm, U256::from(5u8)));
    if norm <= step {
        return Some(());
    }
    let mut p_new = [U256::ZERO; 2];
    let mut xn = xp;
    for (k, (s, o)) in ps.iter().zip(po).enumerate() {
        p_new[k] = udiv_u(au(mu(*s, norm.wrapping_sub(step))?, mu(step, o)?)?, norm);
        xn[k + 1] = udiv_u(mu(xp[k + 1], p_new[k])?, *s);
    }
    let d = crypto_d3::newton_d(p.a, p.gamma, xn)?;
    for xk in xn {
        let frac = du(mu(xk, E18)?, d)?;
        if frac < ten::pow(16) || frac > ten::pow(20) {
            return None;
        }
    }
    let mut x = [du(d, three)?, U256::ZERO, U256::ZERO];
    for (xk, pn) in x.iter_mut().skip(1).zip(p_new) {
        *xk = du(mu(d, E18)?, mu(three, pn)?)?;
    }
    let vp_new = udiv_u(mu(E18, crypto_d3::geometric_mean(x)?)?, t.total_supply);
    // `old_virtual_price > 10**18 and 2 * old_virtual_price - 10**18 > xcp_profit` (L1150-L1153): `and` Vyper
    // укорачивается, как в Python (vyperlang/vyper v0.3.10 docs/types.rst L48-L49; vyper/codegen/expr.py L588-L603) —
    // правая часть (проверяемые операции) считается только при vp_new > 10^18.
    let take = vp_new > E18 && su(mu(U256::from(2u8), vp_new)?, E18)? > xcp_profit;
    // Подстройка принята — p_new упаковывается (L1155): assert p < PRICE_MASK.
    if take && !p_new.iter().all(|v| packable(*v)) {
        return None;
    }
    Some(())
}
