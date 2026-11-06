//! Файл: core/uncapped_entry.rs
//! Что делает: вход в расчёт «без потолка» (решение 141) с боевого пути — собирает «что было в бою» (Battle) и ставит
//! работу в фоновую очередь (uncapped_queue.rs). Только память и короткие блокировки: путь сделки не ждёт.
//! Кандидат: прошёл, но сделка не запущена (сухой режим, монета занята, направление не торгуемо) или отклонён с
//! расчётом (прибыль ниже порога, фильтр финальной котировки, сбой финальной котировки). Отказы до расчёта (нет цены,
//! пула, налоги не проверены, срок) — не ставятся. Сделка: после исполнения покупки — с id сделки.
//! Кто вызывает: core/candidates.rs (cand), core/executor.rs (deal).
//! От чего зависит: uncapped_queue.rs, inner.spreads (начало окна, решение 197), uncapped_types.rs, limits_parts.rs, costs.rs (Calc), dry.rs, settings.
//! Настройки: uncapped.enabled.

use rust_decimal::Decimal;

use crate::core::types::Direction;
use crate::settings::read::At;

use super::costs::Calc;
use super::dry::Mode;
use super::engine::Core;
use super::limits_parts::Parts;
use super::uncapped_queue::submit;
use super::uncapped_state::Key;
use super::uncapped_types::Battle;

/// Включена ли фича (uncapped.enabled = да; не задана — нет). Ошибок нет.
pub fn enabled(core: &Core) -> bool {
    core.inner
        .deps
        .settings
        .flag("uncapped.enabled", At::default())
        .unwrap_or(false)
}

/// Решение 197: начало окна — с какого момента грубый спред направления выше порога (core/signal.rs above_since; у
/// сделки пауза направления ставится только при закрытии, поэтому после покупки время ещё на месте). Ошибок нет.
fn window_from(core: &Core, d: &Direction) -> Option<chrono::DateTime<chrono::Utc>> {
    super::engine::rd(&core.inner.spreads)
        .get(d)
        .and_then(|v| v.above_since)
}

/// Кандидат посчитан: прошёл (`reason` None) или отклонён с расчётом. Составляющие бюджета — сейчас, в режиме расчёта.
/// Принимает: ядро, направление, расчёт (у отказа — лучший), причину отказа, режим. Ошибок нет.
pub fn cand(core: &Core, d: &Direction, calc: &Calc, reason: Option<&str>, mode: &Mode) {
    if !enabled(core) {
        return;
    }
    let battle = Battle {
        verdict: if reason.is_some() { "reject" } else { "accept" }.into(),
        reason: reason.map(str::to_string),
        deal_id: None,
        dry: mode.is_dry(),
        qty: calc.qty,
        invested: calc.spend_max,
        profit: calc.profit_usd,
        budget: calc.size.as_ref().map(|s| s.budget),
        plan_budget: None,
        free: None,
        parts: super::limits_parts::parts(core, d, mode).ok(),
        window_from: window_from(core, d),
    };
    submit(core, Key::Cand(d.clone()), d, battle);
}

/// Вошли в сделку (покупка исполнилась). Принимает: ядро, id сделки, план покупки, бюджет плана до чтения баланса,
/// свободный остаток перед покупкой, составляющие бюджета до открытия сделки. Ошибок нет.
pub fn deal(
    core: &Core,
    id: &str,
    plan: &Calc,
    plan_budget: Decimal,
    free: Option<Decimal>,
    parts: Option<Parts>,
) {
    if !enabled(core) {
        return;
    }
    let battle = Battle {
        verdict: "deal".into(),
        reason: None,
        deal_id: Some(id.to_string()),
        dry: false,
        qty: plan.qty,
        invested: plan.spend_max,
        profit: plan.profit_usd,
        budget: plan.size.as_ref().map(|s| s.budget),
        plan_budget: Some(plan_budget),
        free,
        parts,
        window_from: window_from(core, &plan.direction),
    };
    submit(core, Key::Deal(id.to_string()), &plan.direction, battle);
}
