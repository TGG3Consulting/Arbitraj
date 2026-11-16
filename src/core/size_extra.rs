//! Файл: core/size_extra.rs
//! Что делает: задача t159 — необязательные части подбора объёма (решение 66), не зависящие от потолка прибыли:
//! - решение 342: порог % как ограничение поиска (size.pct_constraint) — выбирается наибольшая прибыль в USDT среди
//!   объёмов, проходящих порог % (ошибка №20: раньше % проверялся только в оптимуме по USDT, profit.rs);
//! - решение 343: участки допустимости (size.feasible_scan) — допустимое множество объёмов может быть несвязным
//!   (minNotional и maxNotional продажи по худшей цене, исполнимость сплита DEX); поиск смотрит и вне отрезка бисекции;
//! - решение 345: мягкий срок подбора (size.soft_deadline_pct от profit.calc_timeout_ms) — по сроку берётся лучшая
//!   посчитанная точка и идёт финальная котировка, кандидат не теряется по общему сроку.
//!
//! Решение 344 (изломы пулов, size.pool_breaks_max) — size_breaks.rs; читается здесь же (одно чтение на подбор).
//! Не заданы — Extra по умолчанию: подбор побайтно как до t159.
//! Кто вызывает: core/size_curve.rs (settings), core/profit.rs (deadline, min_pct), size_search.rs, size_memo.rs.
//! От чего зависит: settings.
//! Настройки: size.pct_constraint, size.feasible_scan, size.pool_breaks_max, size.soft_deadline_pct (вся система,
//! необязательные); profit.calc_timeout_ms (срок, от которого берётся доля).

use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::settings::read::At;
use crate::settings::store::Store;

/// Необязательные части подбора (t159). По умолчанию — всё выключено (как до t159).
#[derive(Debug, Clone, Default)]
pub struct Extra {
    /// Решение 342: порог % направления — ограничение поиска; None — как раньше (оптимум по USDT, % — потом).
    pub min_pct: Option<Decimal>,
    /// Решение 343: искать допустимые объёмы и вне отрезка [наименьший, наибольший] бисекции.
    pub scan: bool,
    /// Решение 343: объёмы ПРОДАЖИ на B (монет), где начинается или кончается участок допустимости по minNotional /
    /// maxNotional по худшей цене стакана B (size_scan.rs regions); пусто — нет биржи на B или выключено.
    pub sell_regions: Vec<Decimal>,
    /// Решение 345: мягкий срок подбора; после него новых точек нет, итог — лучшая посчитанная. None — без срока.
    pub deadline: Option<Instant>,
}

/// Включённые части (одно чтение настроек на подбор).
#[derive(Debug, Clone, Copy, Default)]
pub struct Flags {
    pub pct: bool,
    pub scan: bool,
    /// Решение 344: сколько изломов на пул отдаёт DEX (size_breaks.rs); None — выключено.
    pub pool_breaks: Option<u32>,
    /// Решение 345: доля срока profit.calc_timeout_ms, %; None — без мягкого срока.
    pub soft_pct: Option<Decimal>,
}

/// Что делает: читает необязательные настройки t159. Принимает: настройки. Возвращает: Flags (не задана или неверна —
/// выключено, как до t159). Ошибок нет.
pub fn flags(s: &Store) -> Flags {
    let at = At::default();
    Flags {
        pct: s.flag("size.pct_constraint", at).unwrap_or(false),
        scan: s.flag("size.feasible_scan", at).unwrap_or(false),
        pool_breaks: s
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

/// Что делает: решение 345 — мягкий срок подбора от начала расчёта кандидата. Принимает: настройки, долю (%), начало.
/// Возвращает: момент срока; None — доля или profit.calc_timeout_ms не заданы. Ошибок нет.
pub fn deadline(s: &Store, soft_pct: Option<Decimal>, start: Instant) -> Option<Instant> {
    let pct = soft_pct?;
    let total = s.integer("profit.calc_timeout_ms", At::default()).ok()?;
    let ms = (Decimal::from(total) * pct / Decimal::ONE_HUNDRED)
        .floor()
        .to_u64()?;
    Some(start + Duration::from_millis(ms))
}

/// Текст пояснения выбора при сработавшем мягком сроке (решение 345). Ошибок нет.
pub const LATE: &str =
    "; подбор остановлен мягким сроком size.soft_deadline_pct — взят лучший из посчитанных";

/// Текст пояснения выбора по порогу % (решение 342). Принимает: порог %. Ошибок нет.
pub fn pct_note(min_pct: Decimal) -> String {
    format!(
        "; порог % — ограничение поиска (size.pct_constraint): лучший по USDT среди объёмов с прибылью ≥ {min_pct}%"
    )
}
