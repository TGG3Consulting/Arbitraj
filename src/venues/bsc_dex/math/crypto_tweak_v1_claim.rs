//! Файл: venues/bsc_dex/math/crypto_tweak_v1_claim.rs
//! Что делает: решение 472 (T472-03) — `_claim_admin_fees` после tweak_price пула crypto v1 из двух монет (вынесено из
//! math/crypto_tweak_v1.rs — предел размера файла): gulp фактических балансов после обмена, сбор комиссии админа
//! (frac, mint_relative — d_supply = supply·frac/10^18), newton_D по балансам, get_xcp, деление на totalSupply. Все
//! операции проверяемые — откат обмена при переполнении, делении на ноль, вычитании ниже нуля, несходимости.
//! Кто вызывает: math/crypto_tweak_v1.rs (passes).
//! От чего зависит: core/types (CryptoData, CryptoTweak, CryptoTweakV1), math/crypto_v1_2.rs, crypto_ops.rs, ten.rs,
//! crypto_tweak_v1.rs (Swap).
//! Настройки: нет.
//!
//! Источник: curvefi/curve-factory-crypto 7ed1011 contracts/CurveCryptoSwap2ETH.vy `_claim_admin_fees` L475-L513, xp()
//! L190-L193, get_xcp L469-L472; contracts/CurveTokenV5.vy mint_relative L212-L225. Тот же расчёт у EURS/2CRV
//! («2021-10-30. EURS/CryptoSwap.vy» L541-L575; получатель admin_fee_receiver, токен — константа, gulp — balanceOf).

use alloy_primitives::U256;

use crate::core::types::pool_crypto::{CryptoData, CryptoTweak, CryptoTweakV1};

use super::crypto_ops::{au, du, mu, su};
use super::crypto_tweak_v1::Swap;
use super::crypto_v1_2::{geometric_mean_sorted as gm, newton_d};
use super::ten::{E10, E18};

/// `_claim_admin_fees` (L475-L513) после tweak_price: xcp_profit и virtual_price — значения, только что сохранённые
/// tweak_price (L582, L624 / L634). Принимает: состояние, tweak, v1, обмен, xcp_profit, virtual_price. Возвращает:
/// Some(()) — не откатится; None — откатится. Ошибок нет.
pub fn claim(
    p: &CryptoData,
    t: &CryptoTweak,
    v: &CryptoTweakV1,
    s: Swap,
    xcp_profit: U256,
    vprice: U256,
) -> Option<()> {
    let two = U256::from(2u8);
    let ps = *p.price_scale.first()?;
    // Gulp (L482-L487) — фактические балансы после обмена: вход пришёл (+dx), выход ушёл (−dy).
    let mut g = [*v.gulp.first()?, *v.gulp.get(1)?];
    g[s.i] = au(g[s.i], s.dx)?;
    g[s.j] = su(g[s.j], s.dy)?;
    let mut supply = t.total_supply;
    let profit = xcp_profit;
    if profit > v.xcp_profit_a {
        // fees = (xcp_profit - xcp_profit_a) * self.admin_fee / (2 * 10**10) (L492).
        let fees = du(mu(profit - v.xcp_profit_a, v.admin_fee)?, mu(two, E10)?)?;
        if fees > U256::ZERO && v.fee_receiver {
            // frac = vprice * 10**18 / (vprice - fees) - 10**18 (L496); mint_relative: supply * frac / 10**18.
            let frac = su(du(mu(vprice, E18)?, su(vprice, fees)?)?, E18)?;
            supply = au(supply, du(mu(supply, frac)?, E18)?)?;
            // xcp_profit -= fees*2 (L498): проверяемое вычитание (откат); новое значение дальше в расчёте не
            // участвует (только сохранение xcp_profit / xcp_profit_a — не откатывает).
            su(profit, mu(fees, two)?)?;
        }
    }
    // xp() (L190-L193): [b0 * p0, b1 * p1 * price_scale / PRECISION]; D = newton_D (L505).
    let xp = [
        mu(g[0], *p.precisions.first()?)?,
        du(mu(mu(g[1], *p.precisions.get(1)?)?, ps)?, E18)?,
    ];
    let d = newton_d(p.a, p.gamma, xp)?;
    // get_xcp(D) (L469-L472): [D / N, D * PRECISION / (price_scale * N)], gm(x, True); / total_supply (L508).
    let xq = [du(d, two)?, du(mu(d, E18)?, mu(ps, two)?)?];
    du(mu(E18, gm(xq)?)?, supply)?;
    Some(())
}
