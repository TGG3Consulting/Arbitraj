//! Файл: core/types/transfer.rs
//! Что делает: типы переводов и балансов — сети монеты, вывод, депозит, статусы, баланс.
//! Кто вызывает: ядро (справочник, переводы, учёт), шлюз и адаптеры через контракт.
//! От чего зависит: core/types/market.rs.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::{Asset, VenueId};

/// Данные площадки о монете в одной сети: можно ли ввести и вывести, сколько стоит вывод.
/// Источник — только API площадки.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoinNetworkInfo {
    pub venue: VenueId,
    pub asset: Asset,
    /// Название сети так, как его отдаёт площадка (например «BEP20»).
    pub network: String,
    pub deposit_enabled: bool,
    pub withdraw_enabled: bool,
    /// Комиссия вывода в монетах; None — площадка не сообщила.
    pub withdraw_fee: Option<Decimal>,
    pub withdraw_min: Option<Decimal>,
    pub withdraw_max: Option<Decimal>,
    /// Сколько подтверждений сети ждёт площадка для зачисления депозита.
    pub deposit_confirmations: Option<u32>,
    pub updated_at: DateTime<Utc>,
}

/// Статус перевода (вывода или депозита).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferStatus {
    /// Создан, ещё не отправлен в сеть.
    Pending,
    /// Отправлен в сеть, есть хеш транзакции.
    Sent,
    /// Зачислен получателю.
    Credited,
    /// Отклонён площадкой.
    Rejected,
    /// Отменён.
    Cancelled,
}

/// Запрос на вывод монеты.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawRequest {
    pub venue: VenueId,
    pub asset: Asset,
    pub network: String,
    pub address: String,
    pub amount: Decimal,
    pub deal_id: String,
}

/// Перевод (вывод или депозит) и его текущее состояние.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transfer {
    pub venue: VenueId,
    pub asset: Asset,
    pub network: String,
    pub amount: Decimal,
    pub status: TransferStatus,
    /// Идентификатор на площадке (ID вывода/депозита).
    pub venue_ref: String,
    /// Хеш транзакции в сети; None — ещё не отправлен.
    pub tx_hash: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// Баланс одного актива на площадке или в кошельке.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Balance {
    pub venue: VenueId,
    /// Тикер актива (USDT, BNB, монета).
    pub asset: String,
    pub free: Decimal,
    pub locked: Decimal,
}
