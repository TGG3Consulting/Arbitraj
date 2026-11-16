//! Файл: venues/bsc_dex/check/hook_fold.rs
//! Что делает: решение «335 (лесенка влияния)» — чистые правила лесенки влияния крючка B/C (без узла и хранилища,
//! тестируются на записях узла):
//! - judge_side (ревью 3, Б4): итог стороны замера — квотер отказал на точке («не хватает ликвидности», откат крючка) —
//!   эта точка и большие суммы стороны не измерены; меньшие работают. Не осталось ни одной точки — сторона «не прошла»
//!   (HookImpact.buy_fail / sell_fail), другая сторона работает; не сошлась НАША математика (ревью 3, Н-б) — точка
//!   просто не измерена, отказом квотера не считается;
//! - merge_impact / on_success (ревью 2, Н1): удачная проба перед сделкой — точка на сумме сделки вливается в лесенку
//!   хранилища (накопленный максимум); сторона, которая «не прошла», снова измерена. Лесенки нет (одно число,
//!   hooks.impact_points не задана — ревью 2, Н6) — хранилище не трогается;
//! - on_failure (ревью 2, Б2; ревью 3, Н-в): проба перед сделкой на сумме x не прошла — отказ только этой сделке; в
//!   лесенке этой стороны точки с входом ≥ x убираются (hook_curve::cut), меньшие суммы работают; точек стороны не
//!   осталось — «не прошла» только эта сторона; пул целиком — только если не прошли обе. Лесенки нет — хранилище не
//!   трогается;
//! - local (ревью 2, Н1): лесенка стороны для расчёта сделки = лесенка хранилища + точки ЭТОЙ сделки (merge); без
//!   лесенки — точки сделки, каждая не меньше прежнего одного числа; `lift` (ревью 3, Б3) — нижняя граница влияния
//!   для выбора суммы следующей пробы (exec/hook_drive.rs), в итог сделки не идёт;
//! - covered (ревью 2, Б1): сумма покрыта фактом этой сделки — ближайшая точка с входом ≥ x в лесенке сделки снята
//!   этой сделкой (лесенка сделки неубывающая — merge);
//! - refine (ревью 2, Н3): суммы доуточнения цикла крючков — геометрические середины отрезков соседних точек с
//!   наибольшим скачком влияния; insert — вставка точек без выравнивания (сырые факты одного блока).
//!
//! Кто вызывает: check/hooks_probe.rs (judge_side), exec/trade_probe.rs (on_success, on_failure), exec/hook_drive.rs
//! (local, covered), check/hooks_head.rs (refine, insert); тесты на записях узла.
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

/// Причина «сторона не прошла». Ошибок нет.
pub fn side_fail(i: &HookImpact, buy: bool) -> Option<&String> {
    if buy {
        i.buy_fail.as_ref()
    } else {
        i.sell_fail.as_ref()
    }
}

/// Что делает: итог стороны замера (см. шапку judge_side). Принимает: измеренные точки (по возрастанию входа),
/// отказы квотера (вход, причина), запрошено ли что-то по стороне. Возвращает: (точки ниже наименьшего отказа,
/// причина «не прошла» — точек не осталось при отказе квотера). Ошибок нет.
pub fn judge_side(
    steps: Vec<HookStep>,
    refused: &[(U256, String)],
    asked: bool,
) -> (Vec<HookStep>, Option<String>) {
    let first = refused.iter().min_by_key(|(x, _)| *x);
    let kept: Vec<HookStep> = match first {
        Some((x, _)) => hook_curve::cut(&steps, *x),
        None => steps,
    };
    let fail = match first {
        Some((_, why)) if asked && kept.is_empty() => Some(why.clone()),
        _ => None,
    };
    (kept, fail)
}

/// Что делает: точки пробы вливаются в лесенки прежнего итога (merge) по сторонам; наибольшее влияние стороны
/// пересчитано; сторона с новой точкой снова «прошла». Принимает: прежний итог, новый. Возвращает: итог. Ошибок нет.
pub fn merge_impact(old: &HookImpact, new: &HookImpact) -> HookImpact {
    let side = |was: &[HookStep], add: &[HookStep], pct: Decimal| -> (Vec<HookStep>, Decimal) {
        let mut out = was.to_vec();
        for p in add {
            out = merge(&out, p.clone());
        }
        let top = out.iter().map(|s| s.pct).max().unwrap_or(pct);
        (out, top)
    };
    let fail = |add: &[HookStep], was: &Option<String>| {
        if add.is_empty() { was.clone() } else { None }
    };
    let (buy, buy_pct) = side(&old.buy, &new.buy, old.buy_pct);
    let (sell, sell_pct) = side(&old.sell, &new.sell, old.sell_pct);
    HookImpact {
        buy_pct,
        sell_pct,
        buy_fail: fail(&new.buy, &old.buy_fail),
        sell_fail: fail(&new.sell, &old.sell_fail),
        buy,
        sell,
        refused: Vec::new(),
    }
}

/// Что делает: итог хранилища после удачной пробы перед сделкой (см. шапку). Принимает: прежний итог, итог пробы.
/// Возвращает: новый итог; None — без лесенки, не писать. Ошибок нет.
pub fn on_success(old: &HookImpact, new: &HookImpact) -> Option<HookImpact> {
    // Ревью 4 (Н5): итог не изменился (новой точки нет) — не писать: эпоха котировки и короб допуска не сбрасываются.
    (!old.buy.is_empty() || !old.sell.is_empty())
        .then(|| merge_impact(old, new))
        .filter(|m| m != old)
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
    let top = left.iter().map(|s| s.pct).max();
    let mut i = old.clone();
    let fail = top.is_none().then(|| reason.clone());
    if buy {
        (i.buy, i.buy_fail) = (left, fail);
        i.buy_pct = top.unwrap_or(i.buy_pct);
    } else {
        (i.sell, i.sell_fail) = (left, fail);
        i.sell_pct = top.unwrap_or(i.sell_pct);
    }
    if i.buy_fail.is_some() && i.sell_fail.is_some() {
        return Some(CheckState::Failed { reason });
    }
    Some(CheckState::Known(i))
}

/// Что делает: лесенка стороны для расчёта сделки (см. шапку). Принимает: лесенку хранилища, прежнее одно число (без
/// лесенки), точки этой сделки, нижнюю границу влияния (None — нет). Возвращает: лесенку по возрастанию входа. Ошибок
/// нет.
pub fn local(
    stored: &[HookStep],
    flat: Decimal,
    fresh: &[HookStep],
    lift: Option<Decimal>,
) -> Vec<HookStep> {
    let mut out = stored.to_vec();
    for p in fresh {
        let mut p = p.clone();
        if stored.is_empty() {
            p.pct = p.pct.max(flat);
        }
        out = merge(&out, p);
    }
    if let Some(l) = lift {
        for s in &mut out {
            s.pct = s.pct.max(l);
        }
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
