//! Файл: core/credit_match.rs
//! Что делает: выбор ожидаемого зачисления по сумме события (решение 76): событие подходит к ожиданию, если
//! |поступило − ожидалось| ≤ ожидалось · допуск / 100. Ожидаемый приход считается так же, как в расчёте сделки
//! (вывод минус комиссия вывода и налог перевода в монете — credit_expect.rs), поэтому допуск — только на округление.
//! Подходит ровно одно ожидание — оно; несколько — неоднозначно (решает опрос статуса); ни одного — не наше.
//! Кто вызывает: core/credit_watch.rs; тесты.
//! От чего зависит: rust_decimal.
//! Настройки: допуск — transfer.credit_amount_tolerance_pct (читает credit_watch.rs).

use rust_decimal::Decimal;

/// Итог выбора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Ни одно ожидание не подходит.
    None,
    /// Подходит одно — его номер в списке.
    One(usize),
    /// Подходят несколько (сколько) — по событию не закрываем.
    Many(usize),
}

/// Подходит ли сумма события к ожидаемой. Принимает: ожидалось, поступило, допуск в процентах. Ошибок нет.
pub fn fits(expected: Decimal, got: Decimal, tol_pct: Decimal) -> bool {
    (got - expected).abs() <= expected.abs() * tol_pct / Decimal::ONE_HUNDRED
}

/// Выбирает ожидание. Принимает: ожидаемые приходы открытых ожиданий (того же получателя и монеты), сумму события,
/// допуск в процентах. Возвращает: Pick. Ошибок нет.
pub fn pick(expected: &[Decimal], got: Decimal, tol_pct: Decimal) -> Pick {
    let hits: Vec<usize> = expected
        .iter()
        .enumerate()
        .filter(|(_, e)| fits(**e, got, tol_pct))
        .map(|(i, _)| i)
        .collect();
    match hits.as_slice() {
        [] => Pick::None,
        [one] => Pick::One(*one),
        many => Pick::Many(many.len()),
    }
}
