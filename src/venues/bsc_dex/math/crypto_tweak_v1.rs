//! Файл: venues/bsc_dex/math/crypto_tweak_v1.rs
//! Что делает: решение 472 (T472-03) — модель откатов `tweak_price` пула crypto v1 фабрики Curve (CurveCryptoSwap2ETH,
//! Vyper 0.3.1) после обмена. Пул сначала считает dy, затем зовёт tweak_price(A_gamma, xp после обмена, p, 0) — её
//! raise / assert'ы и проверяемые операции (в исходнике нет unsafe_*) откатывают ВЕСЬ обмен, квотер get_dy их не
//! проверяет. Шаги — как в исходнике, на состоянии чтения (CryptoTweak + CryptoTweakV1):
//!
//! 1. цена сделки p (`_exchange` L747-L753, при dx, dy > 10^5) — иначе last_prices через newton_y (L555-L559);
//! 2. D_unadjusted = newton_D(A, gamma, xp) (L549);
//! 3. virtual_price по xp' = [D/2, D·10^18/(2·price_scale)] (L568-L575), `raise "Loss"` при virtual_price <
//!    old_virtual_price и future_A_gamma_time == 0 (L578-L579; future_A_gamma_time = 0 ⇔ !ramp, у v1 ramp = t > 0);
//! 4. norm, adjustment_step (L585-L590); не not_adjusted — проверяемое `virtual_price * 2 - 10**18` (L595; `and`
//!    Vyper 0.3.1 укорачивается — vyper/old_codegen/expr.py v0.3.1 L960-L969: `if x { y } { 0 }`);
//! 5. подстройка (L599-L627): p_new, newton_D, новый virtual_price; не хватило прибыли — `_claim_admin_fees`;
//!    без подстройки при not_adjusted — тоже `_claim_admin_fees` (L636-L639);
//! 6. `_claim_admin_fees` (L475-L513): gulp фактических балансов (после обмена: вход +dx, выход −dy), сбор комиссии
//!    (frac, mint_relative — CurveTokenV5.vy L212-L225: d_supply = supply·frac/10^18), newton_D по ним, get_xcp
//!    (L469-L472), деление на totalSupply.
//!
//! Оракул — price_oracle() на блоке чтения (internal_price_oracle L516-L527 — то же скользящее среднее, что tweak_price
//! считает на блоке обмена L539-L545): оговорка о времени блока — T472-04. Налог перевода входа не моделируется (gulp
//! входа = +dx). Сохранения состояния (self.*) не откатывают — не моделируются.
//! Тот же текст у CurveCryptoSwap2 без фабрики (EURS/2CRV, curvefi/curve-crypto-contract 41e6289 «deployment-logs/
//! 2021-10-30. EURS/CryptoSwap.vy», Vyper 0.3.0: tweak_price L578-L683, `_claim_admin_fees` L541-L575, exchange
//! L686-L763), отличия (CryptoTweakV1.factory = false; сверка текстов diff): (а) оракул — сохранённая переменная
//! (`price_oracle` L98, геттер без пересчёта), пересчёт на время блока — в чтении (load_crypto_tweak_list.rs);
//! (б) norm и шаг считаются только внутри `if needs_adjustment`, шаг = self.adjustment_step без max(…, norm / 5)
//! (L639-L645); (в) в условии подстройки нет `norm > adjustment_step and old_virtual_price > 0` (L635); (г) без
//! подстройки `_claim_admin_fees` не зовётся (L647, L678-L681); (д) в `_claim_admin_fees`: получатель —
//! admin_fee_receiver пула (L557), токен — константа (L92), gulp — balanceOf обеих монет (L548-L550).
//! Кто вызывает: math/crypto_swap.rs (exchange_out, CryptoMath::TwoV1 при CryptoTweak.v1).
//! От чего зависит: core/types (CryptoData, CryptoTweak, CryptoTweakV1), math/crypto_v1_2.rs, crypto_tweak_v1_claim.rs,
//! crypto_ops.rs, ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/curve-factory-crypto 7ed1011 contracts/CurveCryptoSwap2ETH.vy (строки выше; xp() L190-L193),
//! contracts/CurveTokenV5.vy (mint_relative L212-L225).

use alloy_primitives::U256;

use crate::core::types::pool_crypto::{CryptoData, CryptoTweak, CryptoTweakV1};

use super::crypto_ops::{au, du, mu, su};
use super::crypto_tweak_v1_claim;
use super::crypto_v1_2::{geometric_mean_sorted as gm, newton_d, newton_y};
use super::ten::E18;

/// Обмен, после которого зовётся tweak_price: индексы монет, вход и выход в единицах монет.
#[derive(Debug, Clone, Copy)]
pub struct Swap {
    pub i: usize,
    pub j: usize,
    pub dx: U256,
    pub dy: U256,
}

/// Пройдёт ли tweak_price после обмена (шаги 1–6 шапки). Принимает: состояние пула (A, gamma, price_scale, рампа,
/// precisions), tweak, его часть v1, xp после обмена (`_exchange` L741-L745), обмен. Возвращает: Some(()) — не
/// откатится; None — откатится. Ошибок нет.
pub fn passes(
    p: &CryptoData,
    t: &CryptoTweak,
    v: &CryptoTweakV1,
    xp: [U256; 2],
    s: Swap,
) -> Option<()> {
    let two = U256::from(2u8);
    let ps = *p.price_scale.first()?;
    let po = *t.price_oracle.first()?;
    // 2. D_unadjusted = newton_D(A, gamma, _xp) (L549).
    let d = newton_d(p.a, p.gamma, xp)?;
    // 1. p_i > 0 — цена сделки (её проверяемые операции считает `_exchange` L747-L753); иначе last_prices (L555-L559).
    if price_i(p, s)?.is_zero() {
        let dx_price = du(xp[0], U256::from(1_000_000u32))?;
        let x0 = au(xp[0], dx_price)?;
        let y = newton_y(p.a, p.gamma, [x0, xp[1]], d, 1)?;
        du(mu(ps, dx_price)?, su(xp[1], y)?)?;
    }
    // 3. xp' = [D / N, D * PRECISION / (N * price_scale)] (L568).
    let xq = [du(d, two)?, du(mu(d, E18)?, mu(two, ps)?)?];
    let (mut vp, mut xcp_profit) = (E18, E18);
    if t.virtual_price > U256::ZERO {
        vp = du(mu(E18, gm(xq)?)?, t.total_supply)?;
        xcp_profit = du(mu(t.xcp_profit, vp)?, t.virtual_price)?;
        // raise "Loss" (L578-L579).
        if vp < t.virtual_price && !p.ramp {
            return None;
        }
    }
    // 4. norm = |price_oracle·10^18/price_scale − 10^18| (проверяемое деление на price_scale). Фабрика: до условия,
    // adjustment_step = max(self.adjustment_step, norm / 5) (L585-L590); EURS: только внутри `if needs_adjustment`,
    // adjustment_step = self.adjustment_step без max (EURS/CryptoSwap.vy L639-L645).
    let norm_step = |max5: bool| -> Option<(U256, U256)> {
        let n = du(mu(po, E18)?, ps)?;
        let n = if n > E18 { n - E18 } else { E18 - n };
        let st = if max5 {
            t.adjustment_step.max(n / U256::from(5u8))
        } else {
            t.adjustment_step
        };
        Some((n, st))
    };
    let early = if v.factory {
        Some(norm_step(true)?)
    } else {
        None
    };
    let old = t.virtual_price > U256::ZERO;
    let mut needs = v.not_adjusted;
    if !needs {
        // (virtual_price * 2 - 10**18 > xcp_profit + 2*self.allowed_extra_profit) — проверяемые (L595).
        let lhs = su(mu(vp, two)?, E18)?;
        let rhs = au(xcp_profit, mu(two, t.allowed_extra_profit)?)?;
        // EURS: без `norm > step and old` в условии (EURS/CryptoSwap.vy L635).
        let gate = match early {
            Some((n, st)) => n > st && old,
            None => true,
        };
        needs = lhs > rhs && gate;
    }
    if !needs {
        return Some(());
    }
    let (norm, step) = match early {
        Some(x) => x,
        None => norm_step(false)?,
    };
    // 5. Подстройка (L599-L617): хватило прибыли — выход без _claim_admin_fees.
    if norm > step && old && adjusted(p, t, xp, ps, po, norm, step, xcp_profit)? {
        return Some(());
    }
    // Иначе (L619-L627) и без подстройки (L636-L639) — _claim_admin_fees с virtual_price без подстройки. EURS:
    // без подстройки — выход без _claim_admin_fees (EURS/CryptoSwap.vy L647, L678-L681).
    if !v.factory && !(norm > step && old) {
        return Some(());
    }
    crypto_tweak_v1_claim::claim(p, t, v, s, xcp_profit, vp)
}

/// Цена сделки p (`_exchange` L746-L753): при dx > 10^5 и dy > 10^5 — _dx·10^18/_dy (вход — монета 0) или _dy·10^18/_dx;
/// иначе 0. None — откат (переполнение, деление на ноль). Ошибок нет.
fn price_i(p: &CryptoData, s: Swap) -> Option<U256> {
    let lim = U256::from(100_000u32);
    if s.dx <= lim || s.dy <= lim {
        return Some(U256::ZERO);
    }
    let dx = mu(s.dx, *p.precisions.get(s.i)?)?;
    let dy = mu(s.dy, *p.precisions.get(s.j)?)?;
    if s.i == 0 {
        du(mu(dx, E18)?, dy)
    } else {
        du(mu(dy, E18)?, dx)
    }
}

/// Ветка подстройки (L601-L617). Принимает: состояние, tweak, xp после обмена, price_scale, оракул, norm, шаг,
/// xcp_profit без подстройки. Возвращает: Some(true) — подстройка принята (выход), Some(false) — прибыли не хватило
/// (дальше _claim_admin_fees), None — откат. Ошибок нет.
#[allow(clippy::too_many_arguments)]
fn adjusted(
    p: &CryptoData,
    t: &CryptoTweak,
    xp: [U256; 2],
    ps: U256,
    po: U256,
    norm: U256,
    step: U256,
    xcp_profit: U256,
) -> Option<bool> {
    let two = U256::from(2u8);
    // p_new = (price_scale * (norm - adjustment_step) + adjustment_step * price_oracle) / norm (L601).
    let p_new = du(au(mu(ps, su(norm, step)?)?, mu(step, po)?)?, norm)?;
    // xp = [_xp[0], _xp[1] * p_new / price_scale]; D = newton_D(…, xp) (L604-L607).
    let d = newton_d(p.a, p.gamma, [xp[0], du(mu(xp[1], p_new)?, ps)?])?;
    // xp = [D / N, D * PRECISION / (N * p_new)]; old_virtual_price = 10**18 * gm(xp, True) / total_supply (L608-L610).
    let xq = [du(d, two)?, du(mu(d, E18)?, mu(two, p_new)?)?];
    let vp = du(mu(E18, gm(xq)?)?, t.total_supply)?;
    // (old_virtual_price > 10**18) and (2 * old_virtual_price - 10**18 > xcp_profit) — `and` укорачивается (L614).
    if vp <= E18 {
        return Some(false);
    }
    Some(su(mu(two, vp)?, E18)? > xcp_profit)
}
