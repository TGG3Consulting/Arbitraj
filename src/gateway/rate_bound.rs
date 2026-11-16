//! Файл: gateway/rate_bound.rs
//! Что делает: решение 361 — стык «верх курса стороны DEX без котировок» для отсева кандидата по доказанной границе
//! прибыли (core/sieve.rs). Площадка DEX отдаёт строгий верх средней цены обмена монеты по ВСЕМ пулам, которыми её
//! котировка может пройти (лемма 3 проекта docs/notes/t162-proven-sieve-2026-11-17.md, раздел 2): продажа — не больше
//! ρ USDT за монету на любом объёме, покупка — не больше ρ монет за USDT. Котировок нет — только состояния пулов в
//! памяти адаптера. Метод не действует (NotApplicable с причиной) — ядро кандидата НЕ отсекает и считает, как раньше.
//! Здесь же — счётчики отсева циклов DEX ↔ DEX (адаптер считает, ядро пишет их в сводку dex.quote_count).
//! Кто вызывает: contract.rs (Venue::rate_bound), venues/bsc_dex/quote/bound.rs, core/sieve_side.rs, core/quote_count_log.rs.
//! От чего зависит: rust_decimal.
//! Настройки: нет.

use std::sync::atomic::{AtomicU64, Ordering};

use rust_decimal::Decimal;

/// Строгий верх курса стороны DEX по монете.
#[derive(Debug, Clone, PartialEq)]
pub struct DexBound {
    /// Продажа: USDT за 1 монету; покупка: монет за 1 USDT (в целых единицах монет, decimals учтены). Округлён вверх.
    pub rho: Decimal,
    /// Сколько пулов вошло в набор (для пояснения отсева).
    pub pools: usize,
    /// Наибольшее число хопов маршрута, по которому посчитан верх.
    pub hops: usize,
}

/// Итог запроса верха курса.
#[derive(Debug, Clone, PartialEq)]
pub enum RateBound {
    /// Верх есть — отсев действует.
    Bound(DexBound),
    /// Метод не действует: класс причины (ключ счётчика сводки) и текст.
    NotApplicable { class: &'static str, why: String },
}

impl RateBound {
    /// Что делает: «метод не действует» с классом и текстом. Ошибок нет.
    pub fn na(class: &'static str, why: impl Into<String>) -> RateBound {
        RateBound::NotApplicable {
            class,
            why: why.into(),
        }
    }
}

/// Счётчики отсева циклов DEX ↔ DEX с запуска: [проверено верхом, отсечено (ρ цикла < 1), метод не действует].
static CYCLES: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

/// Что делает: отмечает итог проверки цикла. Принимает: 0 — проверен и пропущен в расчёт, 1 — отсечён, 2 — метод не
/// действует. Ошибок нет (иной индекс игнорируется).
pub fn cycle_count(kind: usize) {
    if let Some(c) = CYCLES.get(kind) {
        c.fetch_add(1, Ordering::Relaxed);
    }
}

/// Счётчики циклов с запуска (см. CYCLES). Ошибок нет.
pub fn cycle_counts() -> [u64; 3] {
    [
        CYCLES[0].load(Ordering::Relaxed),
        CYCLES[1].load(Ordering::Relaxed),
        CYCLES[2].load(Ordering::Relaxed),
    ]
}
