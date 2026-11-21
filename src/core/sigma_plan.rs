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
//! Решение 403 (ревью t188): горячий путь (dirctx.rs → сито каждого направления) — entry_shared: настройки (окно,
//! минимум изменений, граница) читаются ОДИН раз и ВНЕ общего замка рядов `inner.vol`; ключ ряда — тоже вне замка; под
//! замком только один measure_key (поиск, обрезка окна, свёртка); решение и текст отказа — после снятия замка
//! (decide). Запасной путь больше не меряет ряд второй раз — тот же замер. Итог — тот же, что у entry (тест
//! tests/core_sigma_plan_t177.rs; entry — тот же decide).
//! Кто вызывает: core/dirctx.rs (build, момент Entry: план входа, пересчёт перед покупкой, «без потолка», карточка).
//! От чего зависит: core/reserve.rs (opt, at_b; текст отказа — как sigma_now), core/vol.rs (key, measure_key, Miss),
//! core/engine.rs (lk), settings.
//! Настройки: profit.sigma_window_min, profit.sigma_min_changes, profit.sigma_floor_per_s [B] (новых нет).

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{Direction, Side};
use crate::settings::read::At;
use crate::settings::store::Store;

use super::engine::lk;
use super::reserve::{at_b, opt};
use super::vol::{Miss, Volatility};

/// Настройки σ плана, прочитанные один раз (вне замка рядов): окно, минимум изменений, граница [B].
struct Read {
    window: Result<u64, String>,
    min: Result<Option<u64>, String>,
    floor: Result<Option<Decimal>, String>,
}

/// Чтение настроек σ плана. Принимает: хранилище, направление. Ошибок нет (ошибки — в полях).
fn read(s: &Store, d: &Direction) -> Read {
    Read {
        window: s
            .integer("profit.sigma_window_min", At::default())
            .map_err(|e| e.message),
        min: opt(s.integer("profit.sigma_min_changes", At::default())),
        floor: opt(s.decimal("profit.sigma_floor_per_s", at_b(d))),
    }
}

/// Что делает: σ плана входа (см. шапку). Принимает: ряды σ, хранилище, направление, сейчас.
/// Возвращает: σ за секунду (доля). Ошибки: текст reserve.rs sigma_now, если запасной путь (граница при малом числе
/// изменений) не применим.
pub fn entry(
    vol: &mut Volatility,
    s: &Store,
    d: &Direction,
    now: DateTime<Utc>,
) -> Result<Decimal, String> {
    let r = read(s, d);
    let window = r.window.clone()?;
    let key = Volatility::key(&d.sell_on, &d.asset, Side::Sell, &d.buy_on);
    let m = vol.measure_key(&key, window, now);
    decide(r, m, window, d)
}

/// Решение 403 (ревью t188): то же, что entry, по общему замку рядов ядра (`inner.vol`): настройки и ключ ряда — до
/// замка, под замком — только measure_key, решение и текст — после. Принимает: замок рядов, хранилище, направление,
/// сейчас. Возвращает и ошибки: как entry.
pub fn entry_shared(
    vol: &Mutex<Volatility>,
    s: &Store,
    d: &Direction,
    now: DateTime<Utc>,
) -> Result<Decimal, String> {
    let r = read(s, d);
    let window = r.window.clone()?;
    let key = Volatility::key(&d.sell_on, &d.asset, Side::Sell, &d.buy_on);
    let m = lk(vol).measure_key(&key, window, now);
    decide(r, m, window, d)
}

/// Решение по замеру (чистая логика, вне замка) — как reserve.rs sigma_now, а при отказе по profit.sigma_min_changes
/// и границе profit.sigma_floor_per_s [B] > 0 — σ = max(σ ряда, граница) (решение 391). Принимает: настройки, замер,
/// окно, направление. Возвращает: σ за секунду. Ошибки: текст отказа (тот же, что у sigma_now).
fn decide(
    r: Read,
    m: Result<(Decimal, usize), Miss>,
    window: u64,
    d: &Direction,
) -> Result<Decimal, String> {
    let (sigma, changes) = m.map_err(|x| x.text(&d.sell_on, &d.asset, &d.buy_on, window))?;
    if let Some(min) = r.min?
        && (changes as u64) < min
    {
        // Решение 391: неподвижная цена при заданной границе > 0 — σ = max(σ ряда, граница) вместо отказа.
        if let Ok(Some(floor)) = r.floor
            && floor > Decimal::ZERO
        {
            return Ok(sigma.max(floor));
        }
        return Err(format!(
            "волатильность {} на {} (направление с {}) не измерена: за последние {window} мин ненулевых изменений \
             цены {changes}, нужно не меньше {min} (profit.sigma_min_changes)",
            d.asset.ticker, d.sell_on.0, d.buy_on.0
        ));
    }
    Ok(match r.floor? {
        Some(floor) if floor > sigma => floor,
        _ => sigma,
    })
}
