//! Файл: venues/bsc_dex/math/crypto_tweak2.rs
//! Что делает: решение 472 (ревью 442 п. 6, вопрос 3 решения 453) — модель откатов `tweak_price` пула Twocrypto-NG
//! после обмена. Пул сначала считает dy, затем зовёт tweak_price(A_gamma, xp после обмена, 0, K0_prev из get_y) — её
//! assert'ы и проверяемые операции откатывают ВЕСЬ обмен, а квотер пула get_dy их не проверяет. Котировка не должна
//! обещать такой обмен: здесь — те же шаги tweak_price, что могут откатиться, на состоянии чтения (CryptoTweak):
//!
//! 1. D_unadjusted = newton_D(A, gamma, xp, K0_prev) — assert'ы / несходимость (crypto_d2::newton_d_k0);
//! 2. last_prices = get_p(xp, D, A_gamma) · price_scale — assert D и проверяемые операции (crypto_p2.rs);
//! 3. xp' = [D/2, D·10^18/(2·price_scale)], virtual_price = 10^18·isqrt(xp'0·xp'1)/totalSupply, xcp_profit;
//!    вне рампы A/gamma — `assert virtual_price > old_virtual_price` (главный откат малых обменов);
//! 4. подстройка price_scale (vp·2 − 10^18 > xcp_profit + 2·allowed_extra_profit и |oracle/scale − 1| > шага):
//!    p_new, newton_D(xp с p_new, 0), у v2.0.0 — доли x/D в [10^16; 10^20], новый virtual_price.
//!
//! Оракул — price_oracle() пула на блоке чтения: это то же скользящее среднее (internal_price_oracle L1450-L1482),
//! что tweak_price считает на блоке обмена (L847-L876), но на время блока чтения — та же оговорка о времени блока,
//! что у рампы A/gamma (вопрос 1 решения 453). Сохранения состояния (self.*) не моделируются — они не откатывают.
//! `and` Vyper укорачивается (T472-05: vyperlang/vyper v0.3.10 docs/types.rst L48-L49) — правая часть условия
//! считается только при истинной левой.
//! Кто вызывает: math/crypto_swap.rs (exchange_out, Twocrypto-NG при CryptoData.tweak).
//! От чего зависит: core/types (CryptoData, CryptoMath, CryptoTweak), math/crypto_d2.rs, crypto_p2.rs, crypto_ops.rs,
//! math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/twocrypto-ng 89c8dfb (v2.1.0) contracts/main/CurveTwocryptoOptimized.vy — вызов L807
//! (`tweak_price(A_gamma, xp, 0, y_out[1])`), `tweak_price` L813-L996: newton_D L887, get_p L891-L894, xp' L898-L901,
//! virtual_price / assert L906-L921, подстройка L926-L991; d612fde (v2.0.0) — вызов L847, `tweak_price` L853-L1068
//! (assert «Loss» L985, доли x/D L1036-L1038 — только у v2.0.0), остальное — тот же текст.

use alloy_primitives::U256;

use crate::core::types::{CryptoData, CryptoMath, CryptoTweak};

use super::crypto_ops::{au, du, isqrt, mu, su, udiv_u};
use super::ten::{self, E18};
use super::{crypto_d2, crypto_p2};

/// Пройдёт ли tweak_price после обмена. Принимает: состояние пула (A, gamma, price_scale, рампа, версия), состояние
/// tweak_price, xp после обмена (в 18 знаках и ценах пула, как `xp[j] = y` в `_exchange`), K0_prev (второе значение
/// get_y; 0 — ветка _newton_y). Возвращает: Some(()) — не откатится; None — откатится. Ошибок нет.
pub fn passes(p: &CryptoData, t: &CryptoTweak, xp: [U256; 2], k0_prev: U256) -> Option<()> {
    let two = U256::from(2u8);
    let ps = *p.price_scale.first()?;
    let po = *t.price_oracle.first()?;
    // 1. D_unadjusted (L887).
    let d = crypto_d2::newton_d_k0(p.math, p.a, p.gamma, xp, k0_prev)?;
    // 2. MATH.get_p(_xp, D_unadjusted, A_gamma) * price_scale — проверяемое умножение (L891-L894).
    mu(crypto_p2::get_p(xp, d, p.a, p.gamma)?, ps)?;
    // 3. xp' = [unsafe_div(D, 2), D * PRECISION / (N_COINS * price_scale)] (L898-L901).
    let x1 = du(mu(d, E18)?, mu(two, ps)?)?;
    let (mut vp, mut xcp_profit) = (E18, E18);
    if t.virtual_price > U256::ZERO {
        // xcp = isqrt(xp[0] * xp[1]); virtual_price = 10**18 * xcp / total_supply (L908-L909).
        let xcp = isqrt(mu(udiv_u(d, two), x1)?);
        vp = du(mu(E18, xcp)?, t.total_supply)?;
        // xcp_profit = unsafe_div(old_xcp_profit * virtual_price, old_virtual_price) (L911-L914).
        xcp_profit = udiv_u(mu(t.xcp_profit, vp)?, t.virtual_price);
        // Вне рампы: assert virtual_price > old_virtual_price (L919-L921).
        if !p.ramp && vp <= t.virtual_price {
            return None;
        }
    }
    // 4. if virtual_price * 2 - 10**18 > xcp_profit + 2 * allowed_extra_profit (L926).
    let lhs = su(mu(vp, two)?, E18)?;
    let rhs = au(xcp_profit, mu(two, t.allowed_extra_profit)?)?;
    if lhs <= rhs {
        return Some(());
    }
    rebalance(p, t, xp, ps, po)
}

/// Подстройка price_scale (L926-L991): откат возможен в проверяемых операциях, newton_D по xp с p_new и (v2.0.0) в
/// проверке долей x/D. Принимает: состояние, tweak, xp после обмена, price_scale, price_oracle. Возвращает: Some(()) —
/// не откатится; None — откатится. Ошибок нет.
fn rebalance(p: &CryptoData, t: &CryptoTweak, xp: [U256; 2], ps: U256, po: U256) -> Option<()> {
    let two = U256::from(2u8);
    // norm = unsafe_div(unsafe_mul(price_oracle, 10**18), price_scale); |norm − 10**18| (L933-L939).
    let mut norm = udiv_u(po.wrapping_mul(E18), ps);
    norm = if norm > E18 {
        norm.wrapping_sub(E18)
    } else {
        E18.wrapping_sub(norm)
    };
    // adjustment_step = max(rebalancing_params[1], unsafe_div(norm, 5)) (L940-L942).
    let step = t.adjustment_step.max(udiv_u(norm, U256::from(5u8)));
    if norm <= step {
        return Some(());
    }
    // p_new = unsafe_div(price_scale * unsafe_sub(norm, step) + step * price_oracle, norm) (L952-L956).
    let p_new = udiv_u(au(mu(ps, norm.wrapping_sub(step))?, mu(step, po)?)?, norm);
    // xp = [_xp[0], unsafe_div(_xp[1] * p_new, price_scale)] (L961-L963); D = newton_D(…, xp, 0) (L966).
    let xn = [xp[0], udiv_u(mu(xp[1], p_new)?, ps)];
    let d = crypto_d2::newton_d(p.math, p.a, p.gamma, xn)?;
    // v2.0.0: frac = xp[k] * 10**18 / D; assert frac > 10**16 - 1 and frac < 10**20 + 1 (d612fde L1036-L1038).
    if p.math == CryptoMath::TwoNg200 {
        for xk in xn {
            let frac = du(mu(xk, E18)?, d)?;
            if frac < ten::pow(16) || frac > ten::pow(20) {
                return None;
            }
        }
    }
    // xp = [unsafe_div(D, 2), D * PRECISION / (N_COINS * p_new)] (L969-L972); новый virtual_price — unsafe_div, но
    // 10**18 * isqrt(xp[0] * xp[1]) проверяемые (L976-L978).
    let x1 = du(mu(d, E18)?, mu(two, p_new)?)?;
    let vp_new = udiv_u(mu(E18, isqrt(mu(udiv_u(d, two), x1)?))?, t.total_supply);
    // if old_virtual_price > 10**18 and 2 * old_virtual_price - 10**18 > xcp_profit (L981-L984): `and` Vyper
    // укорачивается, как в Python (vyperlang/vyper v0.3.10 docs/types.rst L48-L49; кодогенерация — `if x { y } { 0 }`,
    // vyper/codegen/expr.py L588-L603) — правая часть (проверяемые умножение и вычитание) считается только при
    // vp_new > 10^18, тогда вычитание ниже нуля не уходит; итог условия на откат не влияет.
    if vp_new > E18 {
        su(mu(two, vp_new)?, E18)?;
    }
    Some(())
}
