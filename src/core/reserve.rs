//! Файл: core/reserve.rs
//! Что делает: решение 240 — запас на движение цены за время перевода (ТЗ, шаг 4) и σ, по которой он считается.
//! Разбор docs/notes/price-move-vs-reserve-2026-11-09.md (983 сделки): цена за перевод идёт к закрытию разницы, σ ряда
//! продажи занижена (тихий пул; у 24 сделок σ = 0 — запас 0). Три необязательные настройки, значений нет:
//! - profit.closure_share [площадка B, получатель перевода; решение 242 — сильнее значение площадки и типа направления
//!   сделки, core/venue_dir.rs] — доля разницы цен, которая закрывается за перевод: запас не меньше
//!   доля · (средняя продажи на B − средняя покупки на A) · проданное количество (решение 390, t176; прежде — доля · спред
//!   · выручка: на малых спредах то же, что калибровка доли 09.11; на больших прежняя давала запас больше самой разницы
//!   цен, новая ограничена разницей; данных о больших спредах в калибровке нет); цены — те же, что Calc.buy_avg и
//!   Calc.sell_avg; разница ≤ 0 — не действует;
//! - profit.sigma_floor_per_s [площадка B] — нижняя граница σ (доля за секунду): σ плана = max(σ ряда, граница);
//! - profit.sigma_min_changes [вся система] — меньше стольких ненулевых изменений цены в окне σ — σ не измерена,
//!   котировки направления нет (причина — в журнал кандидата через DirCtx::build).
//!
//! Базовая часть запаса (решение 390, t176): выручка · (1 − e^(−k·σ·√T)) — потеря выручки при падении цены продажи на
//! k·σ·√T по логарифму (σ меряется по логарифмам цены, vol.rs). Прежняя форма k·σ·√T·выручка — её первое приближение:
//! при малом k·σ·√T разница ≈ (k·σ·√T)²/2 выручки, при k·σ·√T > 1 прежняя давала запас больше выручки. Запас никогда
//! не больше выручки. keep считается один раз на контекст направления (keep_of → DirCtx.keep) и берётся расчётом
//! (costs.rs) и отсевом (bound_in.rs: κ_B = max(0, keep − r_B)) из одного поля.
//! Не заданы настройки — σ ряда, σ = 0 при неподвижной цене — измерена.
//! σ с границей и проверкой — только на момент расчёта плана (dirctx.rs build: план, пересчёт перед покупкой, кандидаты
//! «без потолка», карточка); у сделки σ — из плана (решение 237), доля — из копии настроек на момент входа
//! (решение 239, Decided.settings).
//! Кто вызывает: core/sigma_plan.rs (opt, at_b — σ плана входа, решение 391; текст отказа — как sigma_now), core/dirctx.rs
//! (closure_share), core/costs.rs и core/reserve_check.rs (amount),
//! core/dirctx.rs и core/uncapped_recompute_ctx.rs (keep_of, closure_share), core/reserve_check.rs (keep_of).
//! От чего зависит: core/vol.rs (measure), settings, rust_decimal (exp).
//! Настройки: profit.closure_share, profit.sigma_floor_per_s, profit.sigma_min_changes, profit.sigma_window_min.

use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, MathematicalOps};

use crate::core::types::{Direction, Side};
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::settings::venue_dir::VenueDir;

use super::vol::Volatility;

/// Необязательное значение: не задано — None; иное (неверный вид) — ошибка с текстом.
pub(super) fn opt<T>(r: Result<T, GatewayError>) -> Result<Option<T>, String> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(e.message),
    }
}

/// Область «площадка B» (продажа, получатель перевода). Принимает: направление. Ошибок нет.
pub(super) fn at_b(d: &Direction) -> At<'_> {
    At {
        venue: Some(&d.sell_on.0),
        coin: None,
    }
}

/// Что делает: доля закрытия разницы направления — profit.closure_share площадки и типа направления сделки (решение
/// 242, core/venue_dir.rs), иначе площадки B (как раньше).
/// Принимает: хранилище (живое или копия момента входа), направление, пару «площадка + тип» (None — нет).
/// Возвращает: доля или None — не задана нигде. Ошибки: текст — значение хранится в неверном виде.
pub fn closure_share(
    s: &Store,
    d: &Direction,
    vd: Option<VenueDir>,
) -> Result<Option<Decimal>, String> {
    opt(s.decimal_dir("profit.closure_share", vd, at_b(d)))
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

/// Что делает: доля выручки, которая остаётся после базовой части запаса (решение 390, t176): e^(−c), c = k·σ·√T.
/// Принимает: c. Возвращает: e^(−c) ∈ (0, 1] при c ≥ 0; 0 — когда e^c не помещается в Decimal (c > ≈ 66: e^(−c) меньше
/// 10^(−28), ниже разрешения Decimal); c < 0 (недопустимо настройками: k ≥ 0, σ ≥ 0, √T ≥ 0) — 1 − c, как прежняя
/// линейная форма. Ошибок нет. Одна функция для расчёта (amount) и отсева (bound_in.rs) — граница и расчёт согласованы.
pub fn keep(c: Decimal) -> Decimal {
    if c <= Decimal::ZERO {
        return Decimal::ONE - c;
    }
    match c.checked_exp() {
        // e^c ≥ 1 — деление без переполнения.
        Some(e) => Decimal::ONE / e,
        None => Decimal::ZERO,
    }
}

/// Что делает: keep(k·σ·√T) для контекста направления — один раз на DirCtx (ревью t176, Н3: не на каждую точку
/// объёма). Принимает: k, σ, время в пути T (с). Возвращает: e^(−k·σ·√T); None — √T не вычисляется (T < 0).
/// Ошибок нет. Порядок умножения k·σ·√T — как до решения 390.
pub fn keep_of(k: Decimal, sigma: Decimal, transit_s: Decimal) -> Option<Decimal> {
    transit_s.sqrt().map(|r| keep(k * sigma * r))
}

/// Что делает: запас на движение цены в USDT. Принимает: keep = keep_of(k, σ, T) контекста (DirCtx.keep), выручку,
/// долю закрытия (None — не задана), средние цены покупки на A и продажи на B на объёме расчёта. Возвращает:
/// выручка · (1 − keep); с долей и ценой продажи выше цены покупки — большее из него и доля · (продажа − покупка) ·
/// проданное количество (проданное = выручка / средняя продажи); не больше выручки. Ошибок нет.
pub fn amount(
    keep: Decimal,
    proceeds: Decimal,
    share: Option<Decimal>,
    buy_avg: Decimal,
    sell_avg: Decimal,
) -> Decimal {
    let base = proceeds * (Decimal::ONE - keep);
    let Some(share) = share else { return base };
    if buy_avg <= Decimal::ZERO || sell_avg <= buy_avg {
        return base;
    }
    // Цена продажи сходится к цене покупки на долю разницы: потеря = доля · (продажа − покупка) · количество — не больше
    // самой разницы цен (прежняя форма доля · спред · выручка на больших спредах давала больше разницы). Доля ≤ 1
    // (каталог: доля 0–1) ⇒ это меньше выручки; min — та же гарантия при любом хранимом значении.
    let closure = share * (sell_avg - buy_avg) / sell_avg * proceeds;
    base.max(closure).min(proceeds)
}
