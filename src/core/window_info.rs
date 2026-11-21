//! Файл: core/window_info.rs
//! Что делает: сводка окна (решение 125) — строка экрана «Окна» и JSON в SQLite (storage/windows_db.rs): монета,
//! направления, биржи записи и что на них включено, начало и конец, наибольшая прибыль, наши сделки и их итог,
//! счётчики записи, точки цены DEX (решение 241; решение 245 — и на объём сделки), окна максимального объёма
//! направлений (решение 245), отказы предпроверки сделки (решение 266: причина, сколько раз, метка «баланс»),
//! теневые расчёты, пока монета занята сделкой (решение 267, busy_sum.rs), причина закрытия; подписи для журнала и Telegram (pairs, mark).
//! Кто вызывает: core/window.rs (ведёт), window_close.rs, window_view.rs, window_cfg.rs, интерфейс (JSON).
//! От чего зависит: core/types, gateway/tap.rs (TapInfo), uncapped_maxwin.rs, busy_sum.rs, chrono, rust_decimal, serde.
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

/// Решение 266 (ответ Тиграна 13.11): отказ предпроверки сделки в окне — сделка по прошедшему расчёту не состоялась.
/// Повторы одной причины (тот же вид текста без чисел — reject_shape.rs, как в журнале, решение 155) по одному
/// направлению — одна запись со счётчиком.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WinReject {
    pub direction: Direction,
    /// Текст последнего отказа (как в журнале core.precheck после «сделки нет — »).
    pub reason: String,
    /// Вид причины (текст без чисел) — по нему склеиваются повторы.
    pub shape: String,
    pub count: u64,
    pub first_at: DateTime<Utc>,
    pub last_at: DateTime<Utc>,
    /// Отказ из-за свободного баланса A (precheck_balance.rs is_balance).
    pub balance: bool,
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
    /// Решение 241: точек цены пула DEX, отправленных в файл окна (window_dex.rs; решение 245 — и на объём сделки,
    /// window_deal.rs). Старые сводки без поля — 0.
    #[serde(default)]
    pub dex_points: u64,
    /// Решение 245: окна максимального объёма направлений окна (core/uncapped_maxwin.rs): по направлению — самое
    /// длинное, последнее, сколько. Старые сводки — пусто.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maxwins: Vec<super::uncapped_maxwin::DirMaxWins>,
    /// Решение 266 (ответ Тиграна 13.11): отказы предпроверки сделки за время окна (window.rs note_reject). Старые
    /// сводки — пусто.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejects: Vec<WinReject>,
    /// Решение 267: сводки теневых расчётов направлений, пока монета занята сделкой (window.rs note_busy; замер
    /// включается window.busy_calc). Старые сводки и окна без замера — пусто (поле не пишется).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub busy: Vec<super::busy_sum::BusySum>,
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
