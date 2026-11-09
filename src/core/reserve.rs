//! Файл: core/reserve.rs
//! Что делает: решение 240 — запас на движение цены за время перевода (ТЗ, шаг 4) и σ, по которой он считается.
//! Разбор docs/notes/price-move-vs-reserve-2026-11-09.md (983 сделки): цена за перевод идёт к закрытию разницы, σ ряда
//! продажи занижена (тихий пул; у 24 сделок σ = 0 — запас 0). Три необязательные настройки, значений нет:
//! - profit.closure_share [площадка B, получатель перевода] — доля разницы цен, которая закрывается за перевод:
//!   запас = max(k·σ·√T·выручка, доля · спред · выручка); спред — разница цен, которую видит план:
//!   (средняя продажи на B − средняя покупки на A) / средняя покупки на A на объёме расчёта (те же цены, что Calc.buy_avg
//!   и Calc.sell_avg); спред ≤ 0 — вторая часть не действует;
//! - profit.sigma_floor_per_s [площадка B] — нижняя граница σ (доля за секунду): σ плана = max(σ ряда, граница);
//! - profit.sigma_min_changes [вся система] — меньше стольких ненулевых изменений цены в окне σ — σ не измерена,
//!   котировки направления нет (причина — в журнал кандидата через DirCtx::build).
//!
//! Не заданы — ровно как раньше: запас k·σ·√T·выручка, σ ряда, σ = 0 при неподвижной цене — измерена.
//! σ с границей и проверкой — только на момент расчёта плана (dirctx.rs build: план, пересчёт перед покупкой, кандидаты
//! «без потолка», карточка); у сделки σ — из плана (решение 237), доля — из копии настроек на момент входа
//! (решение 239, Decided.settings).
//! Кто вызывает: core/dirctx.rs (sigma_now, closure_share), core/costs.rs (amount),
//! core/uncapped_recompute_ctx.rs (closure_share).
//! От чего зависит: core/vol.rs (measure), settings.
//! Настройки: profit.closure_share, profit.sigma_floor_per_s, profit.sigma_min_changes, profit.sigma_window_min.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{Direction, Side};
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::settings::read::At;
use crate::settings::store::Store;

use super::vol::Volatility;

/// Необязательное значение: не задано — None; иное (неверный вид) — ошибка с текстом.
fn opt<T>(r: Result<T, GatewayError>) -> Result<Option<T>, String> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(e.message),
    }
}

/// Область «площадка B» (продажа, получатель перевода). Принимает: направление. Ошибок нет.
fn at_b(d: &Direction) -> At<'_> {
    At {
        venue: Some(&d.sell_on.0),
        coin: None,
    }
}

/// Что делает: доля закрытия разницы направления — profit.closure_share площадки B.
/// Принимает: хранилище (живое или копия момента входа), направление. Возвращает: доля или None — не задана.
/// Ошибки: текст — значение хранится в неверном виде.
pub fn closure_share(s: &Store, d: &Direction) -> Result<Option<Decimal>, String> {
    opt(s.decimal("profit.closure_share", at_b(d)))
}

/// Что делает: σ продажи на B для плана сейчас: ряд направления по окну profit.sigma_window_min (vol.rs measure),
/// проверка profit.sigma_min_changes, граница profit.sigma_floor_per_s [B].
/// Принимает: ряды σ, хранилище, направление, сейчас. Возвращает: σ за секунду (доля).
/// Ошибки: текст — окно не задано, σ не измерена (замеров нет или ненулевых изменений меньше минимума), неверный вид.
pub fn sigma_now(
    vol: &mut Volatility,
    s: &Store,
    d: &Direction,
    now: DateTime<Utc>,
) -> Result<Decimal, String> {
    let window = s
        .integer("profit.sigma_window_min", At::default())
        .map_err(|e| e.message)?;
    let (sigma, changes) = vol.measure(&d.sell_on, &d.asset, Side::Sell, &d.buy_on, window, now)?;
    if let Some(min) = opt(s.integer("profit.sigma_min_changes", At::default()))?
        && (changes as u64) < min
    {
        return Err(format!(
            "волатильность {} на {} (направление с {}) не измерена: за последние {window} мин ненулевых изменений \
             цены {changes}, нужно не меньше {min} (profit.sigma_min_changes)",
            d.asset.ticker, d.sell_on.0, d.buy_on.0
        ));
    }
    Ok(match opt(s.decimal("profit.sigma_floor_per_s", at_b(d)))? {
        Some(floor) if floor > sigma => floor,
        _ => sigma,
    })
}

/// Что делает: запас на движение цены в USDT. Принимает: k, σ, √T, выручку, долю закрытия (None — не задана), средние
/// цены покупки на A и продажи на B на объёме расчёта. Возвращает: k·σ·√T·выручка; с долей и спредом > 0 — большее из
/// него и доля · спред · выручка. Ошибок нет.
pub fn amount(
    k: Decimal,
    sigma: Decimal,
    sqrt_t: Decimal,
    proceeds: Decimal,
    share: Option<Decimal>,
    buy_avg: Decimal,
    sell_avg: Decimal,
) -> Decimal {
    // Как до решения 240 — то же выражение в том же порядке (без настроек результат побайтно прежний).
    let base = k * sigma * sqrt_t * proceeds;
    let Some(share) = share else { return base };
    if buy_avg <= Decimal::ZERO {
        return base;
    }
    let spread = (sell_avg - buy_avg) / buy_avg;
    if spread <= Decimal::ZERO {
        return base;
    }
    base.max(share * spread * proceeds)
}
