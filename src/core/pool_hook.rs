//! Файл: core/pool_hook.rs
//! Что делает: допуск пулов с крючками V4 (решение 32) — вынесен из core/blacklist.rs ревью 4 лесенки влияния (файл
//! был больше 200 строк; пути blacklist::{pool_admission, pool_sides_off} — реэкспорт):
//! - pool_admission: группа крючка допущена (hooks.allowed_groups), влияние проверено и на наименьшей сумме РАБОТАЮЩЕЙ
//!   стороны не выше hooks.max_impact_b_pct / _c_pct (решение «335 (лесенка влияния)»: допуск к сумме сделки
//!   применяет котировка DEX);
//! - pool_sides_off (ревью 4, Н3): стороны, которые «не прошли» (пул работает другой стороной), — для экрана «Монеты».
//!
//! Кто вызывает: core/blacklist.rs (реэкспорт), core/rebuild_build.rs, core/coins_view.rs.
//! От чего зависит: core/types, settings.
//! Настройки: hooks.allowed_groups, hooks.max_impact_b_pct, hooks.max_impact_c_pct.

use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup, HookImpact, HookStep, Pool, PoolCheck};
use crate::settings::read::At;
use crate::settings::store::Store;

/// Допуск пула по крючку V4 (решение 32). Принимает: пул, его проверку влияния, хранилище.
/// Возвращает: Ok — пул можно использовать; Err(причина) — пул исключён (видно в интерфейсе).
pub fn pool_admission(pool: &Pool, check: Option<&PoolCheck>, s: &Store) -> Result<(), String> {
    let group = match &pool.hooks {
        None => return Ok(()),
        Some(h) if h.group == HookGroup::A => return Ok(()),
        Some(h) => h.group,
    };
    let allowed = s
        .text("hooks.allowed_groups", At::default())
        .map_err(|e| e.message)?;
    // Варианты настройки — «A», «A+B», «A+B+C» (settings/catalog/trade.rs).
    let letter = if group == HookGroup::B { "B" } else { "C" };
    if !allowed.split('+').any(|g| g == letter) {
        return Err(format!(
            "крючок группы {letter} не допущен (hooks.allowed_groups = {allowed})"
        ));
    }
    let name = if group == HookGroup::B {
        "hooks.max_impact_b_pct"
    } else {
        "hooks.max_impact_c_pct"
    };
    let max = s.decimal(name, At::default()).map_err(|e| e.message)?;
    match check.map(|c| &c.impact) {
        None | Some(CheckState::NotChecked | CheckState::OneShare(_)) => Err("влияние крючка ещё не проверено".into()),
        Some(CheckState::Failed { reason }) => Err(format!("проверка крючка не прошла: {reason}")),
        // Решение «335 (лесенка влияния)»: влияние — функция суммы; допуск — к влиянию на сумме сделки (его применяет
        // котировка DEX). Здесь пул закрыт, только если допуск превышен уже на наименьшей измеренной сумме стороны.
        // Ревью 4 лесенки (Н3): сторона «не прошла» в допуске не участвует (её влияние — прежнее число, не замер);
        // пул закрыт целиком, только если обе стороны не прошли (тогда итог — Failed) — сторона видна в pool_sides_off.
        Some(CheckState::Known(i)) if over(i, max) => Err(format!(
            "влияние крючка на наименьшей сумме (покупка {}, продажа {}) выше допуска {max}%",
            least_text(&i.buy, i.buy_pct, &i.buy_fail),
            least_text(&i.sell, i.sell_pct, &i.sell_fail)
        )),
        Some(CheckState::Known(_)) => Ok(()),
    }
}

/// Ревью 4 лесенки (Н3): стороны пула с крючком, которые «не прошли» проверку (квотер отказал уже на наименьшей сумме
/// стороны; пул работает другой стороной) — «покупка: причина» / «продажа: причина» для экрана «Монеты». Принимает:
/// проверку пула. Возвращает: список (пусто — обе стороны работают или проверки нет). Ошибок нет.
pub fn pool_sides_off(check: Option<&PoolCheck>) -> Vec<String> {
    let Some(CheckState::Known(i)) = check.map(|c| &c.impact) else {
        return Vec::new();
    };
    [("покупка", &i.buy_fail), ("продажа", &i.sell_fail)]
        .into_iter()
        .filter_map(|(n, f)| f.as_ref().map(|why| format!("{n} не прошла: {why}")))
        .collect()
}

/// Допуск превышен на наименьшей сумме хотя бы одной работающей стороны. Ошибок нет.
fn over(i: &HookImpact, max: Decimal) -> bool {
    (i.buy_fail.is_none() && least(&i.buy, i.buy_pct) > max)
        || (i.sell_fail.is_none() && least(&i.sell, i.sell_pct) > max)
}

/// Наименьшее влияние стороны текстом («не прошла» — без числа). Ошибок нет.
fn least_text(steps: &[HookStep], flat: Decimal, fail: &Option<String>) -> String {
    match fail {
        Some(_) => "не прошла".into(),
        None => format!("{}%", least(steps, flat)),
    }
}

/// Наименьшее влияние стороны по лесенке (решение «335 (лесенка влияния)»); лесенки нет — одно число. Ошибок нет.
fn least(steps: &[HookStep], flat: Decimal) -> Decimal {
    steps.iter().map(|s| s.pct).min().unwrap_or(flat)
}
