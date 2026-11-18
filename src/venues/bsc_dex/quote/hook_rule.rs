//! Файл: venues/bsc_dex/quote/hook_rule.rs
//! Что делает: решение 406 (t190, Тигран 18.11.2026) — значение лесенки влияния крючка B/C на входе x (вынесено из
//! quote/hook_curve.rs: файл был бы больше 200 строк; пути hook_curve::{at, upper, chord} — реэкспорт). Значение
//! точки — накопленный максимум по ней и меньшим суммам (у любой лесенки), отрицательное — 0. x не больше наименьшей
//! точки — её значение; x выше наибольшей — None («не измерено»). Между соседними точками a1 < x < a2:
//! - upper — значение a2 (правило до решения 406);
//! - chord — хорда (линейная интерполяция по входу);
//! - at (правило котировки) — max(хорда, продолжение наклона участка [a0, a1] от a1 до x), не выше значения a2; на
//!   первом участке (нет a0) — хорда. Запись 18.11 (tests/bsc_dex_hook_curve_t190.rs, прикидка вне бота): ускоряющийся
//!   рост (GAIB 300 USDT: факт 1,933 %, хорда 1,893 %) — хорду держит продолжение наклона (1,967 %); замедляющийся
//!   (BTCMT 80 USDT: факт 3,065 %, хорда 2,611 %) — продолжение наклона 3,077 %; потолок — значение a2.
//!
//! Кто вызывает: quote/hook_curve.rs (HookCurve::value, реэкспорт), тесты.
//! От чего зависит: core/types (HookStep), num.rs (to_decimal).
//! Настройки: нет.

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::HookStep;

/// Что делает: правило котировки на входе `x` (см. шапку, решение 406). Принимает: лесенку (по возрастанию входа),
/// вход. Возвращает: влияние, % (не меньше 0); None — пусто или x больше наибольшего входа. Ошибок нет.
pub fn at(steps: &[HookStep], x: U256) -> Option<Decimal> {
    pick(steps, x, Rule::At)
}

/// Что делает: значение ближайшей большей точки (накопленный максимум). Принимает и возвращает — как at. Ошибок нет.
pub fn upper(steps: &[HookStep], x: U256) -> Option<Decimal> {
    pick(steps, x, Rule::Upper)
}

/// Что делает: хорда между соседними точками (для сравнения в тестах и таблице t190). Как at. Ошибок нет.
pub fn chord(steps: &[HookStep], x: U256) -> Option<Decimal> {
    pick(steps, x, Rule::Chord)
}

/// Правило значения между точками.
#[derive(Clone, Copy, PartialEq)]
enum Rule {
    Upper,
    Chord,
    At,
}

/// Общий разбор (см. шапку): значения точек — накопленный максимум, отрицательные — 0. Ошибок нет.
fn pick(steps: &[HookStep], x: U256, rule: Rule) -> Option<Decimal> {
    if x > steps.last()?.amount_in {
        return None;
    }
    let j = steps.iter().position(|s| s.amount_in >= x)?;
    let run = |k: usize| steps[..=k].iter().map(|s| s.pct.max(Decimal::ZERO)).max();
    let hi = run(j)?;
    let Some(i) = j
        .checked_sub(1)
        .filter(|_| rule != Rule::Upper && steps[j].amount_in > x)
    else {
        return Some(hi);
    };
    let (a1, v1) = (steps[i].amount_in, run(i)?);
    let line = lerp((a1, v1), (steps[j].amount_in, hi), x);
    let ext = match i.checked_sub(1).filter(|_| rule == Rule::At) {
        Some(h) => slope(steps[h].amount_in, run(h)?, (a1, v1), x),
        None => line,
    };
    Some(line.max(ext).min(hi))
}

/// Продолжение наклона участка [a0, a1] (значения v0 ≤ v1) от a1 до x > a1 (решение 406). Число не переводится в
/// Decimal — v1 (ниже хорды, max в pick возьмёт хорду). Ошибок нет.
fn slope(a0: U256, v0: Decimal, (a1, v1): (U256, Decimal), x: U256) -> Decimal {
    grow(v1 - v0, x - a1, a1 - a0).map_or(v1, |g| v1 + g)
}

/// Прирост rise · run / base в Decimal; None — число не переводится в Decimal, переполнение или base = 0. Ошибок нет.
fn grow(rise: Decimal, run: U256, base: U256) -> Option<Decimal> {
    let dec = |u: U256| super::super::num::to_decimal(u, 0);
    rise.checked_mul(dec(run)?)?.checked_div(dec(base)?)
}

/// Линейная интерполяция по входу между точками (a0, v0) и (a1, v1), a0 < x < a1, v0 ≤ v1 (решение 406). Отрицательные
/// значения — 0. Вход не переводится в Decimal (больше 96 бит) — v1 (прежнее правило: не лучше ближайшей большей).
/// Ошибок нет.
fn lerp((a0, v0): (U256, Decimal), (a1, v1): (U256, Decimal), x: U256) -> Decimal {
    let (v0, v1) = (v0.max(Decimal::ZERO), v1.max(Decimal::ZERO));
    share(x - a0, a1 - a0)
        .and_then(|w| (v1 - v0).checked_mul(w))
        .map_or(v1, |p| (v0 + p).max(v0).min(v1))
}

/// Доля part / whole в Decimal; None — число не переводится в Decimal или whole = 0. Ошибок нет.
fn share(part: U256, whole: U256) -> Option<Decimal> {
    let dec = |u: U256| super::super::num::to_decimal(u, 0);
    dec(part)?.checked_div(dec(whole)?)
}
