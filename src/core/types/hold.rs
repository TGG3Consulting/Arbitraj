//! Файл: core/types/hold.rs
//! Что делает: удержания вывода (решение 150, Тигран 04.11.2026; решение 166, Тигран 05.11.2026) — что биржа сейчас
//! держит от вывода, хотя на балансе это видно как свободное (MEXC — предварительно зачисленные депозиты «can be used
//! for trading but cannot be withdrawn»; KuCoin — lockedAmount квоты вывода; Bybit — доступно минус «можно вывести»).
//! Ядро по этому списку решает до покупки, пройдёт ли вывод (core/precheck_withdraw.rs).
//! Решение 166: у площадки может не быть официального способа это узнать — тогда ответ «неизвестно» (unknown), а НЕ
//! «удержаний нет»: ядро такую проверку не считает пройденной по фактам, а только пишет в журнал и не блокирует.
//! Решение 251: WithdrawReady — готов ли вывод купленной монеты (контракт withdraw_ready, core/withdraw_ready.rs).
//! Кто вызывает: gateway/contract.rs (withdraw_hold, withdraw_ready), адаптеры бирж (venues/*/deposit_hold.rs, withdraw_quota.rs …),
//! core/precheck_withdraw.rs.
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

/// Удержания вывода площадки сейчас. Пусто и unknown = None — площадка по своему API сообщила, что ничего не держит.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WithdrawHold {
    pub items: Vec<HoldItem>,
    /// Решение 166: Some(почему) — у площадки нет официального способа узнать, сколько можно вывести (или он не описан
    /// в её документации); items тогда пуст. None — ответ по фактам API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown: Option<String>,
}

impl WithdrawHold {
    /// Ответ по фактам API: список удержаний (пустой — ничего не держится). Ошибок нет.
    pub fn of(items: Vec<HoldItem>) -> Self {
        Self {
            items,
            unknown: None,
        }
    }

    /// Решение 166: «неизвестно» с причиной (нет в документации площадки). Ошибок нет.
    pub fn unknown(why: impl Into<String>) -> Self {
        Self {
            items: vec![],
            unknown: Some(why.into()),
        }
    }

    /// Нет ни одного удержания (в том числе при «неизвестно»). Ошибок нет.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Площадка не может сказать, сколько можно вывести (решение 166). Ошибок нет.
    pub fn is_unknown(&self) -> bool {
        self.unknown.is_some()
    }
}

/// Решение 251: готов ли вывод монеты сразу после покупки (контракт withdraw_ready) — ответ площадки после ожидания.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WithdrawReady {
    /// Some(true) — свободно не меньше суммы вывода; Some(false) — меньше (вывод ядро всё равно отправит, отказ закроет
    /// повтор решения 166); None — площадка доступность не сообщает (по умолчанию), ядро не ждало.
    pub ready: Option<bool>,
    /// Свободно на счёте вывода в момент ответа (в монете площадки); None — неизвестно.
    pub free: Option<rust_decimal::Decimal>,
    /// Откуда ответ: «событие баланса», «REST …» — для хронологии сделки.
    pub source: String,
}
