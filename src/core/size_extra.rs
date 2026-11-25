//! Файл: core/size_extra.rs
//! Что делает: задача t159 (ревью t159) — части подбора объёма (решение 66), не зависящие от потолка прибыли:
//! - решение 342: порог % — ОГРАНИЧЕНИЕ поиска (в логике, без выключателя: ревью t159 — регрессии по USDT нет, решение
//!   о сделке по финальной котировке): цель — наибольшая прибыль в USDT среди объёмов, проходящих оба порога
//!   (size_pct.rs). Пороги кладёт вызывающий (profit.rs: money.min_profit_usd / money.min_profit_pct направления);
//! - решение 343: участки допустимости (в логике): концы склеенных участков по minNotional / maxNotional продажи по
//!   худшей цене стакана B (size_scan.rs regions — точная формула); точки выше наибольшего объёма бисекции — только
//!   для немонотонностей без формулы (сторона DEX) и при size.scan_above_pct (параметр-число);
//! - решение 344: изломы пулов DEX-сторон маршрута лучшей точки — size.pool_breaks_max (общий предел на кандидата,
//!   size_breaks.rs), считаются ПОСЛЕ базового поиска и не вытесняют его;
//! - решение 345: мягкий срок подбора size.soft_deadline_pct — только там, где есть внешний общий срок
//!   profit.calc_timeout_ms (candidates.rs, busy_calc.rs — profit.rs decide_within), действует после границ и только
//!   когда уже посчитан объём, проходящий оба порога (size_gate.rs).
//!
//! Кто вызывает: core/size_curve.rs (search_params, params), core/profit.rs (apply), core/cycle_plan.rs,
//! size_search.rs, size_memo.rs, size_gate.rs.
//! От чего зависит: settings.
//! Настройки: size.scan_above_pct, size.pool_breaks_max, size.soft_deadline_pct (вся система, необязательные);
//! profit.calc_timeout_ms (срок, от которого берётся доля).

use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::settings::read::At;
use crate::settings::store::Store;

/// Части подбора t159. По умолчанию — пусто: порогов нет, участков нет, срока нет (как до t159).
#[derive(Debug, Clone, Default)]
pub struct Extra {
    /// Только DEX → CEX, когда шаг B в единицах A содержит непредставимую дробь.
    pub sell_grid: Option<super::size_unit_grid::SellGrid>,
    /// Exact physical arrival for an ordinary DEX token, when the transfer contract is known.
    pub plateau: Option<super::size_plateau::Arrival>,
    /// Порог прибыли в USDT направления (решение 342: отсечение «лучший по USDT ниже порога — проходящих нет»;
    /// решение 345: признак «проходящий объём найден»). None — неизвестен.
    pub min_usd: Option<Decimal>,
    /// Решение 342: порог % направления — ограничение поиска. None — порога нет (цикл DEX ↔ DEX, «без потолка»).
    pub min_pct: Option<Decimal>,
    /// Решение 343: склеенные участки допустимости ПРОДАЖИ на B (монет; size_scan.rs regions); пусто — B не биржа или
    /// пределов суммы нет.
    pub sell_regions: Vec<super::size_scan::Run>,
    /// Ревью t159 Б3: списание по бюджету монотонно по объёму — A биржа (худшая цена asks не убывает) или цикл (вход ×
    /// курс). Тогда объём выше «слишком большого по бюджету» тоже слишком большой.
    pub budget_monotone: bool,
    /// Ревью t159 Б3: есть сторона DEX — немонотонность без формулы (исполнимость сплита, бюджет умного маршрута).
    pub no_formula: bool,
    /// Решение 343: size.scan_above_pct — на сколько % от наибольшего объёма бисекции искать допустимые объёмы выше
    /// него (size.grid_points равномерных точек). None — выше не ищем.
    pub above_pct: Option<Decimal>,
    /// Решение 344: size.pool_breaks_max — сколько изломов пулов DEX наибольшим числом на кандидата. None — без них.
    pub breaks_max: Option<u32>,
    /// Решение 345: мягкий срок подбора. None — без срока.
    pub deadline: Option<Instant>,
}

/// Необязательные настройки t159 (одно чтение на подбор).
#[derive(Debug, Clone, Copy, Default)]
pub struct Flags {
    pub above_pct: Option<Decimal>,
    pub breaks_max: Option<u32>,
    /// Решение 345: доля срока profit.calc_timeout_ms, %; None — без мягкого срока.
    pub soft_pct: Option<Decimal>,
}

/// Что делает: читает необязательные настройки t159. Принимает: настройки. Возвращает: Flags (не задана или неверна —
/// выключено). Ошибок нет.
pub fn flags(s: &Store) -> Flags {
    let at = At::default();
    Flags {
        above_pct: s
            .decimal("size.scan_above_pct", at)
            .ok()
            .filter(|p| *p > Decimal::ZERO),
        breaks_max: s
            .integer("size.pool_breaks_max", at)
            .ok()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0),
        soft_pct: s
            .decimal("size.soft_deadline_pct", at)
            .ok()
            .filter(|p| *p > Decimal::ZERO && *p < Decimal::ONE_HUNDRED),
    }
}

/// Что делает: пороги, изломы и мягкий срок подбора кандидата (profit.rs). Принимает: настройки, параметры
/// (изменяются), пороги USDT и %, начало внешнего общего срока (None — его нет: предпроверка, refit — мягкого срока
/// нет, ревью t159 Б2). Ошибок нет.
pub fn apply(
    s: &Store,
    p: &mut super::size::Params,
    min: (Decimal, Decimal),
    outer_from: Option<Instant>,
) {
    let f = flags(s);
    p.ext.min_usd = Some(min.0);
    p.ext.min_pct = Some(min.1);
    p.ext.breaks_max = f.breaks_max;
    p.ext.deadline = outer_from.and_then(|from| deadline(s, f.soft_pct, from));
}

/// Что делает: решение 345 — мягкий срок подбора от начала внешнего срока. Принимает: настройки, долю (%), начало.
/// Возвращает: момент срока; None — доля или profit.calc_timeout_ms не заданы. Ошибок нет.
pub fn deadline(s: &Store, soft_pct: Option<Decimal>, start: Instant) -> Option<Instant> {
    let pct = soft_pct?;
    let total = s
        .integer("profit.calc_timeout_ms", At::default())
        .ok()
        .filter(|ms| *ms > 0)?;
    let ms = (Decimal::from(total) * pct / Decimal::ONE_HUNDRED)
        .floor()
        .to_u64()?;
    Some(start + Duration::from_millis(ms))
}

/// Текст пояснения выбора при сработавшем мягком сроке (решение 345). Ошибок нет.
pub const LATE: &str = "; подбор остановлен мягким сроком size.soft_deadline_pct — взят лучший из посчитанных, \
     проходящих оба порога";

/// Текст пояснения выбора, сдвинутого порогом % (решение 342). Принимает: порог %. Ошибок нет.
pub fn pct_note(min_pct: Decimal) -> String {
    format!(
        "; лучший по USDT объём не проходит порог {min_pct}% — взят наибольший по USDT среди проходящих оба порога"
    )
}
