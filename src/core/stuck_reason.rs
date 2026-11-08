//! Файл: core/stuck_reason.rs
//! Что делает: причина, по которой монета ушла в режим застрявшей монеты (решение 220, Тигран 08.11.2026). До решения
//! журнал и Telegram всегда писали «продавать невыгодно», хотя монета попадала сюда и после несостоявшегося перевода,
//! без котировки, после продажи, не ушедшей в сеть, после неисполненного ордера биржи и с остатком после частичной
//! продажи. Теперь каждый вызов stuck::handle передаёт настоящую причину — она в журнале, хронологии и Telegram.
//! Кто вызывает: sell.rs, sell_unsent.rs, executor.rs (передают причину), stuck.rs (пишет её).
//! От чего зависит: gateway/errors.rs (код ошибки продажи), op_error.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::gateway::errors::ErrorCode;

use super::op_error::OpError;

/// Почему монета застряла. Текст ошибки — как его дал шлюз (уже с площадкой).
#[derive(Debug, Clone, PartialEq)]
pub enum StuckReason {
    /// Котировка есть, но итог сделки при продаже сейчас ниже нуля (решение 47).
    Unprofitable,
    /// Котировки нет (нет данных, глубины, кошелька) — текст причины.
    NoQuote(String),
    /// Перевод на площадку продажи не состоялся — текст ошибки.
    TransferFailed(String),
    /// Продажа не ушла в сеть (решения 110, 154) — текст ошибки.
    Unsent(String),
    /// Ордер продажи на бирже не исполнился совсем (решение 182) — текст ошибки.
    NotFilled(String),
    /// Продано не всё — остаток.
    Partial(Decimal),
    /// BNB на DEX: газ продажи не помещается сверх резерва (решение 62) — текст ошибки.
    GasReserve(String),
}

impl StuckReason {
    /// Причина по ошибке продажи, которую повторяет режим застрявшей монеты (sell_unsent.rs): OrderNotFilled —
    /// «ордер не исполнился», остальное — «не ушла в сеть». Принимает: ошибку продажи. Ошибок нет.
    pub fn of_sell(e: &OpError) -> StuckReason {
        if e.code == Some(ErrorCode::OrderNotFilled) {
            StuckReason::NotFilled(e.message.clone())
        } else {
            StuckReason::Unsent(e.message.clone())
        }
    }

    /// Коротко — заголовок Telegram «Монета застряла: …». Ошибок нет.
    pub fn title(&self) -> &'static str {
        match self {
            StuckReason::Unprofitable => "продавать сейчас невыгодно",
            StuckReason::NoQuote(_) => "нет котировки продажи",
            StuckReason::TransferFailed(_) => "перевод не состоялся",
            StuckReason::Unsent(_) => "продажа не ушла в сеть",
            StuckReason::NotFilled(_) => "ордер продажи не исполнился",
            StuckReason::Partial(_) => "продано не всё",
            StuckReason::GasReserve(_) => "газ продажи не помещается сверх резерва BNB",
        }
    }

    /// Полностью — для журнала и хронологии (с текстом ошибки или остатком). Ошибок нет.
    pub fn text(&self) -> String {
        match self {
            StuckReason::Unprofitable => self.title().to_string(),
            StuckReason::Partial(left) => format!("{}, остаток {left}", self.title()),
            StuckReason::NoQuote(e)
            | StuckReason::TransferFailed(e)
            | StuckReason::Unsent(e)
            | StuckReason::NotFilled(e)
            | StuckReason::GasReserve(e) => format!("{}: {e}", self.title()),
        }
    }
}
