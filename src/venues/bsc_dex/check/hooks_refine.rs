//! Файл: venues/bsc_dex/check/hooks_refine.rs
//! Что делает: решение «335 (лесенка влияния)», ревью 2 (Н3, Н6) — доуточнение лесенки цикла крючков (вынесено из
//! check/hooks_head.rs и check/hook_fold.rs ревью 4: файлы были больше 200 строк):
//! - refined: итог первого замера → итог цикла: без лесенки (hooks.impact_points не задана, Н6) — точки убираются
//!   (одно число на все суммы); с лесенкой — второй пакет на том же блоке с точками в отрезках с наибольшим скачком
//!   влияния (Н3), точки вставляются как есть (сырые факты одного блока); не удался — итог первого;
//! - refine: суммы доуточнения — геометрические середины отрезков соседних точек с наибольшим скачком влияния;
//! - insert: вставка точек без выравнивания (точка того же входа заменяется).
//!
//! Кто вызывает: check/hooks_head.rs (refined); check/hook_fold.rs (реэкспорт refine, insert для тестов).
//! От чего зависит: check/hooks_probe.rs (ladder, measure_plan), check/hook_fold.rs (judge_side), check/hook_points.rs
//! (Sides), pools/listen.rs (Pools), core/types.
//! Настройки: нет своих (суммы и число точек — check/hook_points.rs).

use std::sync::Arc;

use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{HookImpact, HookStep};

use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::hook_fold;
use super::hook_points::Sides;
use super::hooks_probe;

/// Что делает: итог первого замера → итог цикла (см. шапку). Принимает: пулы, запись на блоке, суммы, итог первого,
/// блок. Возвращает: итог. Ошибок нет.
pub async fn refined(
    p: &Pools,
    fresh: &Entry,
    sides: &Sides,
    mut i: HookImpact,
    tag: &str,
) -> HookImpact {
    if sides.flat {
        (i.buy, i.sell) = (Default::default(), Default::default());
        i.refused.clear();
        return i;
    }
    let (buy, sell) = (refine(&i.buy, sides.extra), refine(&i.sell, sides.extra));
    if buy.is_empty() && sell.is_empty() {
        i.refused.clear();
        return i;
    }
    let plan = match hooks_probe::ladder(&p.ctx, &p.tokens, &p.reg, fresh, (&buy, &sell)).await {
        Ok(Ok(plan)) => plan,
        _ => {
            i.refused.clear();
            return i;
        }
    };
    if let Ok(Ok(j)) = hooks_probe::measure_plan(&p.ctx, fresh, &plan, tag).await {
        // Отказ квотера на точке доуточнения — она и большие суммы стороны не измерены (как в первом пакете).
        let cut = |b: bool| -> Vec<_> {
            j.refused
                .iter()
                .filter(|r| r.0 == b)
                .map(|r| (r.1, r.2.clone()))
                .collect()
        };
        let (buy, buy_fail) = hook_fold::judge_side(insert(&i.buy, &j.buy), &cut(true), true);
        let (sell, sell_fail) = hook_fold::judge_side(insert(&i.sell, &j.sell), &cut(false), true);
        (i.buy, i.sell) = (Arc::new(buy), Arc::new(sell));
        i.buy_fail = i.buy_fail.take().or(buy_fail);
        i.sell_fail = i.sell_fail.take().or(sell_fail);
        let top = |s: &[HookStep], was: Decimal| s.iter().map(|x| x.pct).max().unwrap_or(was);
        (i.buy_pct, i.sell_pct) = (
            top(i.buy.as_slice(), i.buy_pct),
            top(i.sell.as_slice(), i.sell_pct),
        );
    }
    i.refused.clear();
    i
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
