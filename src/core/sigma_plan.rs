//! Файл: core/sigma_plan.rs
//! Что делает: σ продажи на B для плана входа (задача t177, решение 391 (t177), п. 2). Обычный путь — reserve.rs
//! sigma_now без изменений (ряд направления по окну profit.sigma_window_min, проверка profit.sigma_min_changes, граница
//! profit.sigma_floor_per_s [B]). Новое — только когда sigma_now отказала из-за profit.sigma_min_changes (ряд измерен,
//! но ненулевых изменений цены в окне меньше минимума: неподвижная цена) И граница profit.sigma_floor_per_s [B]
//! задана и больше нуля: σ плана = max(σ ряда, граница) вместо отказа «σ не измерена». Так направление с неподвижной
//! ценой не выпадает из расчёта, а запас не ниже k·граница·√T·выручка.
//! Без изменений: граница не задана (или 0) — отказ, как раньше; ряда нет или в окне нет ни одного шага (новое
//! направление, один замер) — отказ, как раньше (факта о цене за окно нет — «не торгуем вслепую»).
//! Доказательство на данных: 11 сделок живого бота 14–16.11 с σ плана = 0 (ряд в окне не менялся) — у всех движение
//! цены продажи за перевод 0,000 % (docs/notes/scripts/sigma_series/run.py, часть A; заметка
//! docs/notes/t177-sigma-series-2026-11-17.md).
//! Кто вызывает: core/dirctx.rs (build, момент Entry: план входа, пересчёт перед покупкой, «без потолка», карточка).
//! От чего зависит: core/reserve.rs (sigma_now, opt, at_b), core/vol.rs (measure), settings.
//! Настройки: profit.sigma_window_min, profit.sigma_min_changes, profit.sigma_floor_per_s [B] (новых нет).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{Direction, Side};
use crate::settings::read::At;
use crate::settings::store::Store;

use super::reserve::{at_b, opt, sigma_now};
use super::vol::Volatility;

/// Что делает: σ плана входа (см. шапку). Принимает: ряды σ, хранилище, направление, сейчас.
/// Возвращает: σ за секунду (доля). Ошибки: текст sigma_now, если запасной путь (граница при малом числе изменений)
/// не применим.
pub fn entry(
    vol: &mut Volatility,
    s: &Store,
    d: &Direction,
    now: DateTime<Utc>,
) -> Result<Decimal, String> {
    sigma_now(vol, s, d, now).or_else(|e| few_changes(vol, s, d, now).ok_or(e))
}

/// Что делает: запасной путь при малом числе изменений. Принимает: как entry. Возвращает: Some(max(σ ряда, граница)) —
/// ряд измерен, ненулевых изменений меньше profit.sigma_min_changes, граница profit.sigma_floor_per_s [B] > 0;
/// иначе None (настройка не задана или в неверном виде, ряда или шагов нет, изменений достаточно). Ошибок нет.
fn few_changes(
    vol: &mut Volatility,
    s: &Store,
    d: &Direction,
    now: DateTime<Utc>,
) -> Option<Decimal> {
    let floor = opt(s.decimal("profit.sigma_floor_per_s", at_b(d)))
        .ok()
        .flatten()
        .filter(|f| *f > Decimal::ZERO)?;
    let min = opt(s.integer("profit.sigma_min_changes", At::default()))
        .ok()
        .flatten()?;
    let window = s.integer("profit.sigma_window_min", At::default()).ok()?;
    let (sigma, changes) = vol
        .measure(&d.sell_on, &d.asset, Side::Sell, &d.buy_on, window, now)
        .ok()?;
    ((changes as u64) < min).then_some(sigma.max(floor))
}
