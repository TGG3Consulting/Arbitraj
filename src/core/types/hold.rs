//! Файл: core/types/hold.rs
//! Что делает: удержания вывода (решение 150, Тигран 04.11.2026) — что биржа сейчас держит от вывода, хотя на балансе
//! это видно как свободное (например, у MEXC предварительно зачисленные депозиты «can be used for trading but cannot be
//! withdrawn» до полного числа подтверждений). Ядро по этому списку решает до покупки, пройдёт ли вывод (core/precheck_withdraw.rs).
//! Кто вызывает: gateway/contract.rs (withdraw_hold), адаптеры бирж (venues/mexc/deposit_hold.rs), core/precheck_withdraw.rs.
//! От чего зависит: rust_decimal, serde.
//! Настройки: нет.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Одно удержание: монета, сколько и почему (текст для человека).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HoldItem {
    /// Тикер монеты удержания (как у площадки: USDT, BNB, монета).
    pub asset: String,
    /// Сколько монеты удержано.
    pub amount: Decimal,
    /// Почему — для человека, например «предварительно зачислено 45 USDT, подтверждений 12, нужно 20».
    pub reason: String,
}

/// Удержания вывода площадки сейчас. Пусто — ничего не держится (или у площадки такого механизма нет / он не описан).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WithdrawHold {
    pub items: Vec<HoldItem>,
}

impl WithdrawHold {
    /// Нет ни одного удержания. Ошибок нет.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
