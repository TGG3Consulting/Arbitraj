//! Файл: venues/bsc_dex/math/crypto_tweak_v1_3.rs
//! Что делает: решение 472 (T472-03) — модель откатов `tweak_price` tricrypto 2021 (CryptoSwap.vy Vyper 0.2.15,
//! математика — внешний контракт CurveCryptoMath3) после обмена. Шаги — как в исходнике, на состоянии чтения:
//!
//! 1. цена сделки p (`exchange` L677-L689: i, j ≠ 0 — last_prices[i−1]·_dx/_dy; i = 0 — _dx·10^18/_dy; j = 0 —
//!    _dy·10^18/_dx) и номер цены ix (j, при j = 0 — i): last_prices[ix−1] = p (L478-L485); без цены — last_prices
//!    через newton_y по каждой монете (L486-L493);
//! 2. D_unadjusted = newton_D (L472); упаковка last_prices — `assert p < PRICE_MASK` (L495-L500);
//! 3. virtual_price по xp' = [D/3, D·10^18/(3·price_scale_k)] (L506-L518), `raise "Loss"` (L519-L521);
//! 4. не not_adjusted — проверяемое `virtual_price * 2 - 10**18` (L530; `x and y` из двух частей в Vyper 0.2.15 —
//!    `if x { y } { 0 }`, vyper v0.2.15 old_codegen/expr.py L906-L914);
//! 5. подстройка (L533-L577): norm = Σ ratio² (проверяемые ** и +), `norm > adjustment_step ** 2`, sqrt_int, p_new,
//!    newton_D, новый virtual_price, упаковка price_scale — `assert p < PRICE_MASK`. `_claim_admin_fees` tweak_price
//!    tricrypto не зовёт (вызовы — только claim_admin_fees L962 и apply_new_parameters L1092).
//!
//! Оракул — пересчитан на время блока чтения в чтении (pools/load_crypto_tweak_list.rs, math/crypto_math3.rs oracle_at;
//! упаковка оракула L460-L466 — там же): оговорка о времени блока — T472-04.
//! Кто вызывает: math/crypto_swap.rs (exchange_out, CryptoMath::TriV1 при CryptoTweak.v1).
//! От чего зависит: core/types (CryptoData, CryptoTweak, CryptoTweakV1), math/crypto_v1_3.rs, crypto_math3.rs,
//! crypto_ops.rs, ten.rs, crypto_tweak_v1.rs (Swap).
//! Настройки: нет.
//!
//! Источник: curvefi/curve-crypto-contract a0f8401 «deployment-logs/2021-09-13. Arbitrum/CryptoSwap.vy» (пул
//! 0x960ea3e3C7FB317332d990873d354E18d7645590; строки выше; PRICE_SIZE = 128, PRICE_MASK = 2^128 − 1 — L179-L180).

use alloy_primitives::U256;

use crate::core::types::pool_crypto::{CryptoData, CryptoTweak, CryptoTweakV1};

use super::crypto_math3::sqrt_int;
use super::crypto_ops::{au, du, mu, su};
use super::crypto_tweak_v1::Swap;
use super::crypto_v1_3::{geometric_mean_sorted as gm, newton_d, newton_y};
use super::ten::E18;

/// PRICE_MASK = 2^128 − 1 (L180). Ошибок нет.
pub fn price_mask() -> U256 {
    (U256::from(1u8) << 128usize) - U256::from(1u8)
}

/// Пройдёт ли tweak_price после обмена (шаги шапки). Принимает: состояние пула, tweak, его часть v1 (last_prices,
/// not_adjusted), xp после обмена (L671-L674), обмен. Возвращает: Some(()) — не откатится; None — откатится.
pub fn passes(
    p: &CryptoData,
    t: &CryptoTweak,
    v: &CryptoTweakV1,
    xp: [U256; 3],
    s: Swap,
) -> Option<()> {
    let three = U256::from(3u8);
    let ps = [*p.price_scale.first()?, *p.price_scale.get(1)?];
    let po = [*t.price_oracle.first()?, *t.price_oracle.get(1)?];
    let mut last = [*v.last_prices.first()?, *v.last_prices.get(1)?];
    let d = newton_d(p.a, p.gamma, xp)?;
    let (pi, ix) = price_i(p, s, last)?;
    if pi > U256::ZERO {
        // ix > 0 всегда (ix = j, при j = 0 — i ≠ 0): last_prices[ix−1] = p_i (L479-L480).
        last[ix.checked_sub(1)?] = pi;
    } else {
        let dx_price = du(xp[0], U256::from(1_000_000u32))?;
        let x = [au(xp[0], dx_price)?, xp[1], xp[2]];
        let real = |k: usize| -> Option<U256> {
            let y = newton_y(p.a, p.gamma, x, d, k + 1)?;
            du(mu(ps[k], dx_price)?, su(xp[k + 1], y)?)
        };
        last = [real(0)?, real(1)?];
    }
    if last.iter().any(|l| *l >= price_mask()) {
        return None;
    }
    let xq = [
        du(d, three)?,
        du(mu(d, E18)?, mu(three, ps[0])?)?,
        du(mu(d, E18)?, mu(three, ps[1])?)?,
    ];
    let (mut vp, mut xcp_profit) = (E18, E18);
    let old = t.virtual_price > U256::ZERO;
    if old {
        vp = du(mu(E18, gm(xq)?)?, t.total_supply)?;
        xcp_profit = du(mu(t.xcp_profit, vp)?, t.virtual_price)?;
        if vp < t.virtual_price && !p.ramp {
            return None;
        }
    }
    let two = U256::from(2u8);
    let mut needs = v.not_adjusted;
    if !needs {
        let lhs = su(mu(vp, two)?, E18)?;
        needs = lhs > au(xcp_profit, mu(two, t.allowed_extra_profit)?)?;
    }
    if !needs {
        return Some(());
    }
    adjust(p, t, xp, ps, po, xcp_profit, old)
}

/// Цена сделки и номер цены (`exchange` L677-L689). Возвращает: (p, ix) — p = 0 при dx или dy ≤ 10^5; None — откат.
fn price_i(p: &CryptoData, s: Swap, last: [U256; 2]) -> Option<(U256, usize)> {
    let lim = U256::from(100_000u32);
    if s.dx <= lim || s.dy <= lim {
        return Some((U256::ZERO, s.j));
    }
    let dx = mu(s.dx, *p.precisions.get(s.i)?)?;
    let dy = mu(s.dy, *p.precisions.get(s.j)?)?;
    if s.i != 0 && s.j != 0 {
        Some((du(mu(last[s.i - 1], dx)?, dy)?, s.j))
    } else if s.i == 0 {
        Some((du(mu(dx, E18)?, dy)?, s.j))
    } else {
        Some((du(mu(dy, E18)?, dx)?, s.i))
    }
}

/// Ветка подстройки (L533-L577). Принимает: состояние, tweak, xp, price_scale, оракул, xcp_profit, признак
/// «old_virtual_price больше нуля». Возвращает: Some(()) — не откатится; None — откатится.
fn adjust(
    p: &CryptoData,
    t: &CryptoTweak,
    xp: [U256; 3],
    ps: [U256; 2],
    po: [U256; 2],
    xcp_profit: U256,
    old: bool,
) -> Option<()> {
    let three = U256::from(3u8);
    let step = t.adjustment_step;
    // norm += ratio**2, ratio = |price_oracle·10^18/price_scale − 10^18| (L536-L544).
    let ratio = |k: usize| -> Option<U256> {
        let r = du(mu(po[k], E18)?, ps[k])?;
        Some(if r > E18 { r - E18 } else { E18 - r })
    };
    let (r0, r1) = (ratio(0)?, ratio(1)?);
    let norm = au(mu(r0, r0)?, mu(r1, r1)?)?;
    if norm <= mu(step, step)? || !old {
        return Some(());
    }
    // norm = sqrt_int(norm / 10**18); p_new = (ps·(norm − step) + step·po) / norm (L547-L550).
    let norm = sqrt_int(norm / E18)?;
    let pn =
        |k: usize| -> Option<U256> { du(au(mu(ps[k], su(norm, step)?)?, mu(step, po[k])?)?, norm) };
    let p_new = [pn(0)?, pn(1)?];
    // xp[k+1] = _xp[k+1] * p_new[k] / price_scale[k] (L553-L555).
    let x = [
        xp[0],
        du(mu(xp[1], p_new[0])?, ps[0])?,
        du(mu(xp[2], p_new[1])?, ps[1])?,
    ];
    let d = newton_d(p.a, p.gamma, x)?;
    let xq = [
        du(d, three)?,
        du(mu(d, E18)?, mu(three, p_new[0])?)?,
        du(mu(d, E18)?, mu(three, p_new[1])?)?,
    ];
    let vp = du(mu(E18, gm(xq)?)?, t.total_supply)?;
    // (old_virtual_price > 10**18) and (2 * old_virtual_price - 10**18 > xcp_profit) (L567) — затем упаковка p_new.
    if vp <= E18 || su(mu(U256::from(2u8), vp)?, E18)? <= xcp_profit {
        return Some(());
    }
    // Упаковка price_scale: assert p < PRICE_MASK (L569-L573).
    if p_new.iter().any(|q| *q >= price_mask()) {
        return None;
    }
    Some(())
}
