//! Файл: gateway/deposit_min.rs
//! Что делает: решения 370 и 371 (Тигран 17.11, задача t163) — общий разбор поля «минимум депозита» сети у бирж, где
//! «0» — известный минимум 0 («минимума нет, депозит любой»): XT (depositMinAmount), Bitget (minDepositAmount), HTX
//! (minDepositAmt), Phemex (minAmountRv), CoinEx (min_deposit_amount). Документация этих бирж смысл «0» не определяет
//! — это решение владельца, не факт документации (таблица фактов — docs/notes/t163-trade-blockers-2026-11-17.md).
//! Правило:
//! - число ≥ 0 (числом или строкой JSON) — минимум как есть, «0» — минимум 0;
//! - поля нет или null (у Phemex — и пустая строка: его разбор чисел считает её «нет поля») — по
//!   venue.deposit_min_absent_ok площадки (да — 0, иначе неизвестен: сделки с переводом на биржу нет, решение 35);
//! - нечитаемое (не число, меньше нуля) — неизвестен при любой настройке (повторное ревью t163, Н7: мусор не
//!   превращается в «минимума нет»), сеть не выбрасывается, причина — в журнал справочника сетей.
//!
//! Кто вызывает: venues/{xt,bitget,htx,phemex,coinex}/networks_parse.rs.
//! От чего зависит: rust_decimal.
//! Настройки: нет (значение venue.deposit_min_absent_ok передаёт адаптер).

use rust_decimal::Decimal;

/// Что пришло в поле минимума депозита.
#[derive(Debug, Clone, PartialEq)]
pub enum Raw {
    /// Число ≥ 0.
    Value(Decimal),
    /// Поля нет или null.
    Absent,
    /// Не прочитано — причина (текст разбора адаптера или «меньше нуля»).
    Unreadable(String),
}

/// Что делает: вид значения по итогу разбора числа адаптером. Принимает: Ok(None) — поля нет, Ok(Some) — число,
/// Err — текст ошибки разбора. Возвращает: Raw. Ошибок нет.
pub fn classify(parsed: Result<Option<Decimal>, String>) -> Raw {
    match parsed {
        Ok(Some(m)) if m >= Decimal::ZERO => Raw::Value(m),
        Ok(Some(m)) => Raw::Unreadable(format!("{m} меньше нуля")),
        Ok(None) => Raw::Absent,
        Err(e) => Raw::Unreadable(e),
    }
}

/// Что делает: минимум депозита сети. Принимает: вид значения, venue.deposit_min_absent_ok площадки. Возвращает:
/// минимум; None — неизвестен (сделки с переводом на биржу нет). Ошибок нет.
pub fn resolve(raw: &Raw, absent_ok: bool) -> Option<Decimal> {
    match raw {
        Raw::Value(m) => Some(*m),
        Raw::Absent => absent_ok.then_some(Decimal::ZERO),
        Raw::Unreadable(_) => None,
    }
}

/// Что делает: пояснение для журнала справочника сетей, если значение не прочитано. Принимает: имя поля, вид.
/// Возвращает: текст или None. Ошибок нет.
pub fn note(field: &str, raw: &Raw) -> Option<String> {
    match raw {
        Raw::Unreadable(why) => Some(format!(
            "{field} не прочитан ({why}) — минимум депозита неизвестен, перевода на биржу нет"
        )),
        _ => None,
    }
}
