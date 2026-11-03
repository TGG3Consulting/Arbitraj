//! Файл: core/window_info.rs
//! Что делает: сводка окна (решение 125) — строка экрана «Окна» и JSON в SQLite (storage/windows_db.rs): монета,
//! направления, биржи записи и что на них включено, начало и конец, наибольшая прибыль, наши сделки и их итог,
//! счётчики записи, причина закрытия; подписи для журнала и Telegram (pairs, mark).
//! Кто вызывает: core/window.rs (ведёт), window_close.rs, window_view.rs, window_cfg.rs, интерфейс (JSON).
//! От чего зависит: core/types, gateway/tap.rs (TapInfo), chrono, rust_decimal, serde.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{Asset, Direction, VenueId};
use crate::gateway::tap::TapInfo;

/// Площадка, которая пишется в окне.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WinVenue {
    pub venue: VenueId,
    /// Монета, как её знает эта площадка (тикер направления).
    pub asset: Asset,
    /// С какого момента пишется (площадка вошла с более поздним направлением — позже начала окна).
    pub since: DateTime<Utc>,
    /// Что включил адаптер; Err — запись площадки не включилась (причина).
    pub tap: Option<Result<TapInfo, String>>,
}

/// Наша сделка в окне.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WinDeal {
    pub id: String,
    pub open: bool,
    /// Шаг сделки (Closed, Halted…).
    pub step: String,
    /// Итог, USDT (у закрытой).
    pub result_usd: Option<Decimal>,
}

/// Сводка окна — строка экрана «Окна» (SQLite, storage/windows_db.rs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: String,
    pub asset: Asset,
    /// Открыто в сухом режиме.
    pub dry: bool,
    /// Все направления, проходившие расчёт за время окна.
    pub directions: Vec<Direction>,
    pub venues: Vec<WinVenue>,
    pub started_at: DateTime<Utc>,
    /// None — окно открыто.
    pub ended_at: Option<DateTime<Utc>>,
    /// Наибольшая прибыль на бюджете (расчёт кандидата) и без бюджета (карточка сигнала), USDT.
    pub max_profit: Decimal,
    pub max_profit_unbounded: Option<Decimal>,
    pub deals: Vec<WinDeal>,
    /// Итог сделок: «plus» / «minus» / «zero»; None — сделки не было или итога нет.
    pub outcome: Option<String>,
    /// Кадров записано, потеряно (приёмник полон), размер файла, байт.
    pub frames: u64,
    pub lost: u64,
    pub bytes: u64,
    /// Запись остановлена (лимит места) — почему.
    pub stopped: Option<String>,
    pub close_reason: Option<String>,
}

/// Направления окна текстом. Ошибок нет.
pub fn pairs(i: &WindowInfo) -> String {
    let d: Vec<String> = i
        .directions
        .iter()
        .map(|d| format!("{} → {}", d.buy_on.0, d.sell_on.0))
        .collect();
    format!("{} ({})", i.asset.ticker, d.join(", "))
}

/// Пометка сухого режима. Ошибок нет.
pub fn mark(i: &WindowInfo) -> &'static str {
    if i.dry { "[сухой] " } else { "" }
}
