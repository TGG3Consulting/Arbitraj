//! Файл: core/ride_shift.rs
//! Что делает: решение 480 (ревью 2) — сдвиг продажи направления ОДНИМ чтением на расчёт: монеты наших идущих сделок
//! групп, которые ещё продадут на площадке продажи (решение 464, правило Тиграна 20.11: ride_quote::sell_ahead), плюс
//! остаток наших продаж с неизвестным исходом там (худший случай, ride_settled::worst), строка хронологии о худшем
//! случае и признак «котировка не как раньше» (live). Его снимает costs.rs в начале расчёта (Source::shift) и передаёт
//! котировке, доле глубины и худшей цене продажи — все части одного расчёта на одном сдвиге; partial_back.rs — для
//! «продать на A».
//! Кто вызывает: core/costs_source.rs (Live::shift), core/ride_quote.rs (ahead), core/partial_back.rs.
//! От чего зависит: sale_marks.rs (sales), ride_quote.rs (idle, sell_ahead), ride_settled.rs (worst, text), engine.rs,
//! core/types.
//! Настройки: нет (базовое поведение).

use rust_decimal::Decimal;

use crate::core::types::Direction;

use super::engine::Core;

/// Сдвиг продажи одного расчёта.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shift {
    /// true — котировка продажи идёт веткой ride_quote::live_sell (граница своих продаж, свежесть, сдвиг); false —
    /// как раньше (снимок, теневой замер; групп нет и продаж с неизвестным исходом нет).
    pub live: bool,
    /// Монет впереди нашей продажи (группы + худший случай); 0 — нет.
    pub off: Decimal,
    /// Строка хронологии о худшем случае (ride_settled::text); None — продаж с неизвестным исходом нет.
    pub note: Option<String>,
    /// Ревью 3: счётчик итогов наших продаж монеты на площадке (sale_marks::sales), снятый ДО количеств сдвига; его
    /// сверяет ride_settled::after после котировки — продажа, кончившаяся между чтением сдвига и концом котировки, даёт
    /// отказ (окна нет).
    pub seq: u64,
}

/// Что делает: снять сдвиг продажи направления (шапка). Принимает: ядро, направление. Возвращает: Shift.
/// Ошибки: текст — как у ride_settled::worst (запись сделки с неизвестным исходом не найдена).
pub fn read(core: &Core, d: &Direction) -> Result<Shift, String> {
    // Ревью 3: счётчик — первым; итог продажи после него (учёт уменьшен, набор сменился) увидит after.
    let seq = super::sale_marks::sales(core, &d.sell_on, &d.asset);
    let (unsure, ids) = super::ride_settled::worst(core, d)?;
    let idle = super::ride_quote::idle(core, d);
    let rides = if idle {
        Decimal::ZERO
    } else {
        super::ride_quote::sell_ahead(core, d)
    };
    Ok(Shift {
        live: !idle || !ids.is_empty(),
        off: rides + unsure,
        note: super::ride_settled::text(d, unsure, &ids),
        seq,
    })
}
