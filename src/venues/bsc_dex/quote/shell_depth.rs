//! Файл: venues/bsc_dex/quote/shell_depth.rs
//! Что делает: решение 486 — оценки пула Shell Protocol (Proteus) для графа: спот (монет за сумму встречного — по
//! малой пробе формулой пула), пустота (заморожен или баланс стороны меньше MIN_BALANCE реализации), комиссия в
//! процентах (округление входа и выхода: ≈ 2 / BASE_FEE — «fee is applied twice», комментарий BASE_FEE развёрнутых
//! реализаций) и глубина в полосе — по средней цене исполнения формулой пула (как стейблсвап, quote/stable_depth.rs):
//! наибольший вход, при котором средняя цена хуже спота не больше чем на полосу (бисекция). Комиссия округлением в
//! формуле неотделима — глубина считается с ней (отличие от V2: там без комиссии).
//! Кто вызывает: quote/{spot, emptiness, steps, depth_band}.rs.
//! От чего зависит: math/proteus_swap.rs, math/proteus_curve.rs (min_balance), core/types (ProteusData), num.rs.
//! Настройки: нет (полоса — dex.depth_band_pct, передаёт depth_band.rs).

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::ProteusData;

use super::super::math::full_math::mul_div;
use super::super::math::proteus_curve::min_balance;
use super::super::math::proteus_swap::{from_ocean, min_input, swap_given_input, to_ocean};

/// Шагов бисекции глубины (точность 2^-64 диапазона) и удвоений верхней границы — пределы расчёта, не пороги.
const BISECT: usize = 64;
const GROW: usize = 40;
/// Масштаб доли полосы (18 знаков) — точность расчёта, не порог (как quote/depth_band.rs).
const SCALE: f64 = 1e18;
/// Проба спота — во столько раз больше наименьшего входа реализации (точность цены, не порог).
const PROBE_MUL: u64 = 1000;

/// Выход в единицах учёта за вход в единицах учёта (балансы пула на блоке). None — откат формулы. Ошибок нет.
fn out(d: &ProteusData, in_x: bool, input: U256) -> Option<U256> {
    swap_given_input(d, d.x_bal, d.y_bal, input, in_x)
}

/// Проба спота (единицы учёта) и выход за неё. None — пул не отвечает. Ошибок нет.
fn probe(d: &ProteusData, in_x: bool) -> Option<(U256, U256)> {
    let bal = if in_x { d.x_bal } else { d.y_bal };
    let p = min_input(bal).checked_mul(U256::from(PROBE_MUL))?;
    let o = out(d, in_x, p)?;
    (!o.is_zero()).then_some((p, o))
}

/// Монет за сумму встречного токена по споту пробы. Принимает: состояние, монету, сумму встречного (единицы токена).
/// Возвращает: монет (единицы токена) или None — нет цены / монета не из пула. Ошибок нет.
pub fn spot(d: &ProteusData, coin: Address, amount: U256) -> Option<U256> {
    let coin_x = d.is_x(coin)?;
    let in_x = !coin_x;
    let (dec_in, dec_out) = if in_x { (d.dec_x, d.dec_y) } else { (d.dec_y, d.dec_x) };
    let (p, o) = probe(d, in_x)?;
    let got = mul_div(to_ocean(amount, dec_in)?, o, p)?;
    from_ocean(got, dec_out, d.unwrap_fee_divisor)
}

/// Пуст ли пул: заморожен или баланс стороны меньше MIN_BALANCE (Proteus.sol L19, _findSlice откатит). Ошибок нет.
pub fn empty(d: &ProteusData) -> bool {
    let min = min_balance().into_raw();
    d.frozen || d.x_bal < min || d.y_bal < min
}

/// Комиссия в процентах: 2 · 100 / BASE_FEE (вход и выход округляются на сумму / BASE_FEE). None — BASE_FEE 0 или
/// не в u64. Ошибок нет.
pub fn fee_pct(d: &ProteusData) -> Option<Decimal> {
    let f: u64 = d.base_fee.try_into().ok()?;
    Decimal::from(200u32).checked_div(Decimal::from(f))
}

/// Вход годится: средняя цена o / a не хуже keep · o_p / p (keep — доля 1e18). Ошибок нет.
fn within(d: &ProteusData, in_x: bool, a: U256, (p, o_p): (U256, U256), keep: U256) -> bool {
    let Some(o) = out(d, in_x, a) else {
        return false;
    };
    // o / a ≥ keep · o_p / p  ⇔  o · p / a ≥ o_p · keep / 1e18 (mul_div — без переполнения).
    let one = super::super::math::ten::pow10(18);
    match (mul_div(o, p, a), mul_div(o_p, keep, one)) {
        (Some(l), Some(r)) => l >= r,
        _ => false,
    }
}

/// Глубина одного направления в полосе (как depth_band::side): (сумма во встречном токене — покупка: отдано в пул,
/// продажа: получено; граница найдена до предела удвоений). Принимает: состояние, токен входа, полосу (доля),
/// покупка ли. Ошибки (текст): токен не из пула, нет спота, перевод сумм.
pub fn side(d: &ProteusData, token_in: Address, b: f64, buy: bool) -> Result<(U256, bool), String> {
    let in_x = d.is_x(token_in).ok_or("Shell: токен входа не из пула")?;
    let sp = probe(d, in_x).ok_or("Shell: спот пула не считается")?;
    // Продажа: средняя цена ≥ спот · (1 − b); покупка: отдано за единицу ≤ (1 + b) / спот.
    let keep = if buy { 1.0 / (1.0 + b) } else { 1.0 - b };
    let keep = U256::from((keep * SCALE) as u128);
    let mut lo = sp.0;
    let mut hi = (if in_x { d.x_bal } else { d.y_bal }).max(lo);
    let mut full = false;
    for _ in 0..GROW {
        if !within(d, in_x, hi, sp, keep) {
            full = true;
            break;
        }
        lo = hi;
        hi = hi.checked_mul(U256::from(2u8)).ok_or("Shell: переполнение суммы")?;
    }
    if full {
        for _ in 0..BISECT {
            let mid = lo + (hi - lo) / U256::from(2u8);
            if mid == lo {
                break;
            }
            if within(d, in_x, mid, sp, keep) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
    }
    let o = out(d, in_x, lo).ok_or("Shell: расчёт глубины невозможен")?;
    let (dec_in, dec_out) = if in_x { (d.dec_x, d.dec_y) } else { (d.dec_y, d.dec_x) };
    let units = if buy {
        lo / super::super::math::ten::pow10(u64::from(18u8.saturating_sub(dec_in)))
    } else {
        from_ocean(o, dec_out, d.unwrap_fee_divisor).ok_or("Shell: делитель комиссии вывода 0")?
    };
    Ok((units, full))
}
