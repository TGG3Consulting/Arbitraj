//! Файл: venues/bsc_dex/check/reprobe_rule.rs
//! Что делает: решение 335 (ревью) — чистые правила перемера влияния крючка перед паузой сверки с квотером
//! (check/quoter_reprobe.rs), вынесены, чтобы файл перемера был не больше 200 строк и правила проверялись тестом:
//! - safe: АСИММЕТРИЯ (решение 261: «ниже квотера — безопасно, выше — опасно»). Перемер может снять или не поставить
//!   паузу ТОЛЬКО когда первая сверка разошлась в безопасную сторону — наш выход (за вычетом влияния) НИЖЕ квотера.
//!   Наш выход ВЫШЕ квотера (фактически получим меньше расчёта) или цифр нет — пауза как до решения 335, без перемера,
//!   на обеих сторонах (покупка и продажа) и при любом знаке влияния: перемер на той же сумме и том же блоке по
//!   построению переписал бы любое такое расхождение во «влияние крючка» (ревью t156, п. 2.1);
//! - only_side: из перемера берётся влияние ТОЛЬКО сверяемой стороны, другая остаётся прежней (иначе перемер покупки
//!   переписал бы во «влияние» расхождение продажи, в том числе опасное);
//! - merge: итог после повторной сверки; note: пометка в причине паузы.
//!
//! Кто вызывает: check/quoter_reprobe.rs, tests/bsc_dex_hook_reprobe_335.rs.
//! От чего зависит: check/quoter_compare.rs (Cmp), core/types (HookImpact, Side).
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{HookImpact, HookStep, Side};
use crate::gateway::errors::GatewayResult;

use super::super::quote::hook_curve::merge as merge_step;
use super::quoter_compare::Cmp;

/// Пометка причины: влияние на сторону отрицательное, вычитается 0 (вопрос t156 (г)).
pub const NEGATIVE: &str = "влияние крючка на эту сторону отрицательное";

/// Пометка причины: расхождение в опасную сторону — перемер не применяется (ревью t156, главное).
pub const DANGER: &str = "перемер влияния крючка (hooks.reprobe_before_pause) не применяется: наш выход выше квотера \
     или цифр расхождения нет — пауза как без перемера";

/// Расхождение первой сверки в безопасную сторону: пауза с цифрами и наш выход (за вычетом влияния) меньше выхода
/// квотера. Принимает: итог первой сверки. Возвращает: true — перемер допустим. Ошибок нет.
pub fn safe(first: &Cmp) -> bool {
    matches!(first, Cmp::Pause { nums: Some(n), .. } if n.local < n.remote)
}

/// Лесенка влияния стороны (решение «335 (лесенка влияния)»), по возрастанию входа. Ошибок нет.
pub fn side_steps(i: &HookImpact, side: Side) -> &[HookStep] {
    if side == Side::Buy { &i.buy } else { &i.sell }
}

/// Влияние на сторону, %. Ошибок нет.
pub fn side_pct(i: &HookImpact, side: Side) -> Decimal {
    if side == Side::Buy {
        i.buy_pct
    } else {
        i.sell_pct
    }
}

/// Прежнее влияние с заменой сверяемой стороны на перемеренную. Решение «335 (лесенка влияния)»: у перемера с
/// точками стороны — точки вливаются в прежнюю лесенку стороны (quote/hook_curve.rs merge, накопленный максимум), а
/// влияние стороны — наибольшее по лесенке; без точек — как раньше, одно число. Другая сторона — прежняя.
/// Принимает: прежнее, перемер, сторону. Ошибок нет.
pub fn only_side(old: &HookImpact, new: &HookImpact, side: Side) -> HookImpact {
    let mut i = old.clone();
    let (steps, pct) = if side == Side::Buy {
        (&mut i.buy, &mut i.buy_pct)
    } else {
        (&mut i.sell, &mut i.sell_pct)
    };
    let add = side_steps(new, side);
    if add.is_empty() {
        *pct = side_pct(new, side);
        return i;
    }
    for p in add {
        *steps = merge_step(steps, p.clone());
    }
    *pct = steps.iter().map(|s| s.pct).max().unwrap_or(*pct);
    // Ревью 3: сторона снова измерена — «не прошла» снимается.
    if side == Side::Buy {
        i.buy_fail = None;
    } else {
        i.sell_fail = None;
    }
    i.refused.clear();
    i
}

/// Что делает: итог после перемера. Повторная сверка — пауза: пауза с цифрами повторной сверки и пометкой перемера
/// (и «отрицательное», если влияние стороны меньше 0); иная причина или прошла — она; ошибка повторной сверки —
/// первая пауза с пометкой. Принимает: первую паузу, итог повторной сверки, влияние после перемера, сторону, блок.
/// Возвращает: Cmp. Ошибок нет.
pub fn merge(
    first: Cmp,
    second: GatewayResult<Cmp>,
    i: &HookImpact,
    side: Side,
    block: u64,
) -> Cmp {
    match second {
        Ok(Cmp::Pause {
            block: b,
            why,
            nums,
        }) => {
            let pct = side_pct(i, side);
            let neg = if pct < Decimal::ZERO {
                format!(
                    "; {NEGATIVE} ({}%): квотер с крючком даёт больше нашей математики без крючка, вычитается 0 (quote/quoter.rs less_hook, решение 261)",
                    pct.round_dp(4)
                )
            } else {
                String::new()
            };
            Cmp::Pause {
                block: b,
                why: format!(
                    "{why} (после перемера влияния крючка на этом же блоке {block}: покупка {}%, продажа {}%{neg})",
                    i.buy_pct.round_dp(4),
                    i.sell_pct.round_dp(4)
                ),
                nums,
            }
        }
        Ok(other) => other,
        Err(err) => note(
            first,
            &format!(
                "повторная сверка после перемера не удалась: {}",
                err.message
            ),
        ),
    }
}

/// Пауза с дописанной причиной (иной итог — как есть). Ошибок нет.
pub fn note(c: Cmp, add: &str) -> Cmp {
    match c {
        Cmp::Pause { block, why, nums } => Cmp::Pause {
            block,
            why: format!("{why}; {add}"),
            nums,
        },
        other => other,
    }
}
