//! Файл: venues/bsc_dex/math/stable_ema.rs
//! Что делает: решение 472 (доделка) — проверка `save_p` после обмена у plain-пулов фабрики Curve с оракулом цены
//! (Plain2BasicEMA, Plain2ETHEMA; StableData.ema). `exchange` пула после расчёта выхода (выход — тот же, что у
//! Plain2Basic: math/stableswap_exchange.rs) вызывает `save_p(xp, amp, D)` с xp[i] = x, xp[j] = y (y — без комиссии)
//! и D до обмена; `save_p_from_price` при цене ≠ 0 упаковывает её `pack_prices` — `assert p1 < 2**128` откатывает
//! ВЕСЬ обмен, а квотер пула get_dy этого не проверяет. Вторая упакованная цена — `_ma_price()`: взвешенное среднее
//! прежних упакованных значений (каждое < 2^128, веса alpha и 10^18 − alpha, alpha ≤ 10^18) — < 2^128 всегда;
//! `exp(power)` при power ≤ 0 (power = −dt·10^18/ma_exp_time): raise «exp overflow» только при power ≥ 1,35·10^20;
//! convert(p / q, uint256) в exp (откат при отрицательном частном) здесь не моделируется — это функция exp
//! Remco Bloemen с положительным частным на своей области, проверкой на записи (симуляция exchange, tests/
//! arbitrum_curve_plain_live.rs) ловится; ma_exp_time ≠ 0 (initialize — 866, set_ma_exp_time — `assert
//! _ma_exp_time != 0`). Модель проверяет `_get_p` (проверяемые умножения и деления Vyper 0.3.7) и p ≥ 2^128.
//! Кто вызывает: quote/pool_quote.rs (ветка StableSwap при StableData.ema).
//! От чего зависит: math/stableswap_v1.rs (xp, get_d_t, get_y_t, Template), math/ten.rs.
//! Настройки: нет.
//!
//! Источник: curvefi/curve-factory 5272fd2 contracts/implementations/plain-2/Plain2BasicEMA.vy (Vyper 0.3.7; файл
//! последний раз менялся в dfdf855): ma_exp_time L129, pack_prices L314-L317, _get_p L432-L438, exp L452-L484,
//! _ma_price L488-L500, save_p_from_price L511-L519, save_p L522-L527, exchange L738-L800 (save_p — L775),
//! set_ma_exp_time L1059-L1063; Plain2ETHEMA.vy того же коммита — те же функции (_get_p L438, exchange L744-L802,
//! save_p — L781).

use alloy_primitives::U256;

use super::stableswap_v1::{Template, get_d_t, get_y_t, precision, xp};
use super::ten::E18;

/// Пройдёт ли save_p после обмена dx монеты i → монета j. Принимает: i, j, dx, балансы, rates, A (A·A_PRECISION),
/// шаблон пула. Возвращает: Some(()) — не откатится; None — откатится (или пул не из двух монет: реализации с
/// оракулом — только двухмонетные). Ошибок нет.
pub fn passes(
    i: usize,
    j: usize,
    dx: U256,
    balances: &[U256],
    rates: &[U256],
    amp: U256,
    t: &Template,
) -> Option<()> {
    let mut xp = xp(balances, rates)?;
    if xp.len() != 2 {
        return None;
    }
    // exchange: x = xp[i] + _dx * rates[i] / PRECISION; D = get_D(xp, amp); y = get_y(i, j, x, xp, amp, D).
    let x = xp
        .get(i)?
        .checked_add(dx.checked_mul(*rates.get(i)?)? / precision())?;
    let d = get_d_t(&xp, amp, t)?;
    let y = get_y_t(i, j, x, &xp, amp, t)?;
    // xp[i] = x; xp[j] = y; save_p(xp, amp, D).
    xp[i] = x;
    xp[j] = y;
    let p = get_p(&xp, amp, d, t.amp_precision)?;
    // save_p_from_price: if last_price != 0 → pack_prices(last_price, _ma_price()): assert p1 < 2**128.
    if !p.is_zero() && p >= (U256::from(1u8) << 128) {
        return None;
    }
    Some(())
}

/// `_get_p(xp, amp, D)` двухмонетного пула (Plain2BasicEMA.vy L430-L438): ANN = amp·N; Dr = D/N^N; Dr = Dr·D/xp[k]
/// по монетам; 10^18·(ANN·xp0/A_PRECISION + Dr·xp0/xp1) / (ANN·xp0/A_PRECISION + Dr). Проверяемые операции Vyper:
/// переполнение и деление на ноль — откат. Принимает: xp (две монеты), A, D, A_PRECISION. Возвращает: цену или None.
fn get_p(xp: &[U256], amp: U256, d: U256, ap: U256) -> Option<U256> {
    let n = U256::from(2u8);
    let ann = amp.checked_mul(n)?;
    let mut dr = d.checked_div(U256::from(4u8))?;
    for x in xp {
        dr = dr.checked_mul(d)?.checked_div(*x)?;
    }
    let (x0, x1) = (*xp.first()?, *xp.get(1)?);
    let a = ann.checked_mul(x0)?.checked_div(ap)?;
    let num = a.checked_add(dr.checked_mul(x0)?.checked_div(x1)?)?;
    let den = a.checked_add(dr)?;
    E18.checked_mul(num)?.checked_div(den)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// В равновесии (xp0 = xp1) цена `_get_p` = 10^18: ANN·x/AP + Dr·x/x = ANN·x/AP + Dr (числитель = знаменатель).
    #[test]
    fn balanced_price_is_one() {
        let x = U256::from(1_000_000u64) * E18;
        let d = U256::from(2u8) * x;
        let p = get_p(&[x, x], U256::from(20_000u32), d, U256::from(100u8));
        assert_eq!(p, Some(E18));
    }
}
