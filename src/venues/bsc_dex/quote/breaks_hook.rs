//! Файл: venues/bsc_dex/quote/breaks_hook.rs
//! Что делает: решение 344 (изломы для подбора объёма) × «335 (лесенка влияния)», ревью 3 — ступени лесенки влияния
//! крючка B/C пула шага монеты как изломы выхода: на входе точки лесенки у влияния в котировке излом (quote/hook_curve.rs
//! at — с решения 406 линейная интерполяция между точками, наклон меняется в точке), подбор объёма должен такие объёмы
//! видеть. Перевод входа
//! точки в объём монеты стороны — как у изломов пула (quote/breaks.rs):
//! - продажа: вход точки — монета на пул доли ⇒ q = x / (доля · keep);
//! - покупка: вход точки — встречный токен; монета доли — выход пула на этом входе (наша математика по памяти, без
//!   узла) · keep ⇒ q = выход · keep / доля. Расчёт неполный — точка не отдаётся.
//!
//! Кто вызывает: quote/breaks.rs.
//! От чего зависит: quote/select.rs (лесенка в допуске), quote/pool_quote.rs, num.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::Side;

use super::super::num;
use super::super::pools::registry::Entry;
use super::Quoting;

/// Что делает: объёмы монеты стороны на ступенях лесенки (см. шапку). Принимает: окружение котировки, пул, сторону,
/// decimals USDT, (доля, keep), decimals монеты. Возвращает: объёмы (без порядка); пул не допущен или без лесенки —
/// пусто. Ошибок нет.
pub fn steps(
    q: &Quoting,
    e: &Entry,
    side: Side,
    usdt_dec: u8,
    (share, k): (Decimal, Decimal),
    dec: u8,
) -> Vec<Decimal> {
    let Ok(u) = super::select::usable(q, e, side, usdt_dec) else {
        return Vec::new();
    };
    let Some(steps) = u.hook.steps else {
        return Vec::new();
    };
    if share <= Decimal::ZERO || k <= Decimal::ZERO {
        return Vec::new();
    }
    steps
        .iter()
        .filter_map(|s| match side {
            Side::Sell => num::to_decimal(s.amount_in, dec).map(|x| x / (share * k)),
            Side::Buy => {
                let out = super::pool_quote::swap(&q.ctx, e, e.usdt, true, s.amount_in).ok()?;
                let x = num::to_decimal(out.amount_out, dec).filter(|_| out.complete)?;
                Some(x * k / share)
            }
        })
        .collect()
}
