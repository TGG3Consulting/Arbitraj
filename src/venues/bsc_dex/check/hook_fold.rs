//! Файл: venues/bsc_dex/check/hook_fold.rs
//! Что делает: решение «335 (лесенка влияния)», ревью второго прохода — чистые правила лесенки влияния крючка B/C
//! (без узла и хранилища, тестируются на записях узла):
//! - on_success (ревью Н1): удачная проба перед сделкой — точка на сумме сделки вливается в лесенку хранилища
//!   (quote/hook_curve.rs merge_impact, накопленный максимум). Лесенки нет (одно число, hooks.impact_points не задана —
//!   ревью Н6) — хранилище не трогается;
//! - on_failure (ревью Б2): проба перед сделкой на сумме x не прошла (квотер: «не хватает ликвидности», откат
//!   крючка) — отказ только этой сделке; в лесенке этой стороны точки с входом ≥ x убираются (hook_curve::cut: суммы
//!   от x — «не измерено»), меньшие суммы и другая сторона работают. Пул целиком «не прошла» — только если не осталось
//!   ни одной точки стороны (отказ на наименьшей точке). Лесенки нет — хранилище не трогается (отказ только сделке);
//! - local (ревью Н1): лесенка стороны для расчёта сделки = лесенка хранилища + точки ЭТОЙ сделки (merge) — факт на
//!   сумме сделки идёт в расчёт напрямую, а не через хранилище (где его мог не принять более новый блок цикла);
//!   без лесенки — точки сделки, каждая не меньше прежнего одного числа;
//! - covered (ревью Б1): сумма покрыта фактом этой сделки — ближайшая точка с входом ≥ x в лесенке сделки снята этой
//!   сделкой (влияние на x не больше факта на этой сумме — для неубывающего влияния; лесенка сделки неубывающая —
//!   merge). Не покрыта — ещё проба на x (exec/hook_trade.rs);
//! - refine (ревью Н3): суммы доуточнения цикла крючков — геометрические середины отрезков соседних точек с
//!   наибольшим скачком влияния; отрезки без скачка не делятся; insert — вставка точек без выравнивания (сырые факты).
//!
//! Кто вызывает: exec/trade_probe.rs (on_success, on_failure), exec/hook_trade.rs (local, covered), check/hooks_head.rs
//! (refine, insert); тесты на записях узла.
//! От чего зависит: quote/hook_curve.rs, core/types (HookImpact, HookStep, CheckState).
//! Настройки: нет.

use alloy_primitives::U256;
use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{CheckState, HookImpact, HookStep};

use super::super::quote::hook_curve::{self, merge};

/// Лесенка стороны итога (buy = покупка: вход — встречный токен). Ошибок нет.
pub fn side(i: &HookImpact, buy: bool) -> &[HookStep] {
    if buy { &i.buy } else { &i.sell }
}

/// Что делает: итог хранилища после удачной пробы перед сделкой (см. шапку). Принимает: прежний итог, итог пробы.
/// Возвращает: новый итог; None — без лесенки, не писать. Ошибок нет.
pub fn on_success(old: &HookImpact, new: &HookImpact) -> Option<HookImpact> {
    (!old.buy.is_empty() || !old.sell.is_empty()).then(|| hook_curve::merge_impact(Some(old), new))
}

/// Что делает: итог хранилища после отказа пробы перед сделкой на сумме x (см. шапку). Принимает: прежний итог,
/// сторону, вход пробы, причину. Возвращает: новый итог; None — без лесенки, не писать. Ошибок нет.
pub fn on_failure(
    old: &HookImpact,
    buy: bool,
    x: U256,
    reason: String,
) -> Option<CheckState<HookImpact>> {
    let steps = side(old, buy);
    if steps.is_empty() {
        return None;
    }
    let left = hook_curve::cut(steps, x);
    let Some(top) = left.iter().map(|s| s.pct).max() else {
        return Some(CheckState::Failed { reason });
    };
    let mut i = old.clone();
    if buy {
        (i.buy, i.buy_pct) = (left, top);
    } else {
        (i.sell, i.sell_pct) = (left, top);
    }
    Some(CheckState::Known(i))
}

/// Что делает: лесенка стороны для расчёта сделки (см. шапку). Принимает: лесенку хранилища, прежнее одно число
/// (без лесенки), точки этой сделки. Возвращает: лесенку по возрастанию входа. Ошибок нет.
pub fn local(stored: &[HookStep], flat: Decimal, fresh: &[HookStep]) -> Vec<HookStep> {
    let mut out = stored.to_vec();
    for p in fresh {
        let mut p = p.clone();
        if stored.is_empty() {
            p.pct = p.pct.max(flat);
        }
        out = merge(&out, p);
    }
    out
}

/// Что делает: покрыта ли сумма x фактом этой сделки (см. шапку). Принимает: лесенку сделки (local), точки этой
/// сделки, вход. Возвращает: true — покрыта. Ошибок нет.
pub fn covered(local: &[HookStep], fresh: &[HookStep], x: U256) -> bool {
    local
        .iter()
        .find(|s| s.amount_in >= x)
        .is_some_and(|s| fresh.iter().any(|f| f.amount_in == s.amount_in))
}

/// Что делает: суммы доуточнения, USDT (см. шапку). Принимает: лесенку стороны (точки с суммой USDT), сколько точек
/// добавить. Возвращает: суммы по возрастанию. Ошибок нет.
pub fn refine(steps: &[HookStep], k: usize) -> Vec<Decimal> {
    let mut gaps: Vec<(Decimal, Decimal)> = steps
        .windows(2)
        .filter_map(|w| {
            let jump = (w[1].pct - w[0].pct).abs();
            if jump.is_zero() || w[0].usd <= Decimal::ZERO {
                return None;
            }
            let mid = (w[0].usd * w[1].usd).sqrt()?;
            (mid > w[0].usd && mid < w[1].usd).then_some((jump, mid))
        })
        .collect();
    gaps.sort_by_key(|g| std::cmp::Reverse(g.0));
    let mut out: Vec<Decimal> = gaps.into_iter().take(k).map(|g| g.1).collect();
    out.sort();
    out
}

/// Что делает: точки в лесенку без выравнивания (сырые факты одного блока; точка того же входа заменяется).
/// Принимает: лесенку, точки. Возвращает: по возрастанию входа. Ошибок нет.
pub fn insert(steps: &[HookStep], add: &[HookStep]) -> Vec<HookStep> {
    let mut out: Vec<HookStep> = steps
        .iter()
        .filter(|s| add.iter().all(|a| a.amount_in != s.amount_in))
        .cloned()
        .chain(add.iter().cloned())
        .collect();
    out.sort_by_key(|s| s.amount_in);
    out
}
