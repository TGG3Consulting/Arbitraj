//! Файл: core/types/control.rs
//! Что делает: статистика газа по монете (решение 36), застрявшие монеты и ручное управление
//! (решение 37), команды человека из интерфейса и Telegram.
//! Кто вызывает: ядро (ведёт), интерфейс (экраны «Статистика газа», «Ручное управление»),
//! уведомления (кнопки Telegram превращаются в команды).
//! От чего зависит: core/types/market.rs, rust_decimal.
//! Настройки: stuck.* (режим и пороги), coin.blocked* (ручной чёрный список).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::{Asset, VenueId};

/// Статистика газа по монете (решение 36). Сгоревший газ в расчёт сделки не входит — только сюда.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoinGasStats {
    pub asset: Asset,
    /// Сколько обменов отправлено.
    pub attempts: u64,
    /// Сколько прошло успешно.
    pub successes: u64,
    /// Сколько отменено (откат, не попала в блок, отменено правилом шлюза).
    pub cancels: u64,
    /// Газ на неудачных обменах, BNB.
    pub gas_wasted_bnb: Decimal,
    /// Газ всего, BNB.
    pub gas_total_bnb: Decimal,
    /// Итог по монете: прибыль минус убыток сделок минус газ впустую, USDT.
    pub result_usdt: Decimal,
    pub updated_at: DateTime<Utc>,
}

/// Монета дошла, а продавать невыгодно, и убыток больше stuck.max_loss_pct — ждём решения человека
/// (режим «Порог убытка», решение 37). Алерт в Telegram с кнопками «Продать сейчас» и «Беру на себя».
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StuckAlert {
    /// Идентификатор алерта — UUID (36 знаков): его несут кнопки Telegram (callback_data ≤ 64 байт) и интерфейса.
    pub id: String,
    pub deal_id: String,
    pub venue: VenueId,
    pub asset: Asset,
    pub qty: Decimal,
    /// Убыток при продаже по рынку сейчас, %.
    pub loss_pct: Decimal,
    pub at: DateTime<Utc>,
}

/// Монеты, которые человек забрал себе («Беру на себя»): списаны из учёта бота, бот работает дальше.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualHolding {
    pub id: String,
    pub deal_id: String,
    pub venue: VenueId,
    pub asset: Asset,
    /// Сколько монет было на момент «Беру на себя».
    pub qty: Decimal,
    /// Причина (из алерта или введённая человеком).
    pub reason: String,
    pub taken_at: DateTime<Utc>,
}

/// Откуда пришла команда.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandSource {
    /// Интерфейс.
    Ui,
    /// Кнопка Telegram — только из настроенного чата notify.telegram_chat (решение 37).
    Telegram { chat: String },
}

/// Команда человека ядру.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManualAction {
    /// «Продать сейчас» по алерту.
    SellNow { alert_id: String },
    /// «Беру на себя» по алерту.
    TakeOver { alert_id: String },
    /// «Вернуть боту» — возврат по реальному балансу площадки.
    ReturnToBot { holding_id: String },
}

/// Команда с источником — для журнала.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualCommand {
    pub action: ManualAction,
    pub source: CommandSource,
    /// Кто нажал: у Telegram — пользователь (id и @username, как его описал notify); у интерфейса — None
    /// (авторизации веб-сервера пока нет). Для журнала.
    #[serde(default)]
    pub by: Option<String>,
    pub at: DateTime<Utc>,
}

/// Канал команд человека ядру: notify (кнопки Telegram) кладёт команду, ядро выполняет (core/commands.rs).
/// Отправка — try_send: очередь полна — нажавший видит «команда не передана, нажмите ещё раз».
pub type CommandTx = tokio::sync::mpsc::Sender<ManualCommand>;
