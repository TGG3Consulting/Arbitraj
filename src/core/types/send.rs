//! Файл: core/types/send.rs
//! Что делает: каналы отправки транзакций в сеть (открытая отправка, MEV Guard, билдеры) —
//! описание канала, его состояние и статистика; транзакция в сети и её исход.
//! Кто вызывает: адаптер DEX (создаёт), ядро (газ впустую, учёт), интерфейс (экран «Каналы отправки»).
//! От чего зависит: alloy-primitives, rust_decimal.
//! Настройки: channel.*, send.* (docs/SETTINGS.md); ключи каналов — секреты channel.<id>.key.
//!
//! Решения 33, 41–45: канал — модуль с общим контрактом; способ одиночная/бандл — настройка канала;
//! попадание в блок бот проверяет сам через свой узел; по каналу — отправлено, в блоке, откатов, газ впустую.

use alloy_primitives::B256;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Способ отправки (решение 43).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SendMethod {
    /// Одна транзакция (`eth_sendRawTransaction` или аналог канала).
    Single,
    /// Бандл из одной транзакции без разрешения на откат (`eth_sendBundle` или аналог канала).
    Bundle,
}

impl SendMethod {
    /// Варианты настройки channel.method — в том же порядке, что и способы (стык: каталог настроек
    /// берёт их отсюда, интерфейс сопоставляет способ канала с вариантом через это значение).
    pub const CHOICES: &'static [&'static str] = &["single", "bundle"];

    /// Значение настройки channel.method для способа. Ошибок нет.
    pub const fn setting_value(self) -> &'static str {
        match self {
            SendMethod::Single => Self::CHOICES[0],
            SendMethod::Bundle => Self::CHOICES[1],
        }
    }
}

/// Описание канала — что умеет по официальной документации. Даёт адаптер DEX.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendChannelInfo {
    /// Идентификатор канала — id области настроек «канал» (например «club48»).
    pub id: String,
    /// Название для интерфейса.
    pub title: String,
    /// Какие способы отправки есть у канала по его документации.
    pub methods: Vec<SendMethod>,
    /// Закрытый (приватный) канал; false — открытая отправка в публичный пул.
    pub private: bool,
    /// Есть ли у канала чаевые билдеру по документации.
    pub tips: bool,
    /// Имя секрета ключа канала (secrets::KNOWN); None — ключ не нужен.
    pub secret: Option<String>,
    /// Ссылки на официальную документацию канала.
    pub docs: Vec<String>,
}

/// Состояние канала сейчас.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ChannelState {
    /// Включён и готов.
    Ready,
    /// Выключен настройкой channel.enabled.
    Disabled,
    /// Нужен ключ, а он не введён (решение 41: канал не используется).
    NoKey,
    /// Не задана нужная настройка — какая именно.
    NotConfigured { reason: String },
    /// Канал отказывает — последняя причина как есть.
    Failing { reason: String },
}

/// Статистика и состояние канала (решение 42). Счётчики — с запуска учёта (хранит адаптер).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendChannelStats {
    pub id: String,
    pub state: ChannelState,
    /// Сколько транзакций (бандлов) канал принял.
    pub sent: u64,
    /// Сколько канал отклонил (ответ-ошибка).
    pub rejected: u64,
    /// Сколько из отправленных попало в блок (по своему узлу).
    pub included: u64,
    /// Сколько попало в блок с откатом.
    pub reverted: u64,
    /// Газ, сгоревший на откатах, в BNB.
    pub gas_wasted_bnb: Decimal,
    pub updated_at: DateTime<Utc>,
}

/// Исход транзакции в сети (по своему узлу, решение 42).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxOutcome {
    /// Отправлена, квитанции ещё нет.
    Pending,
    /// В блоке, успех (`status = 0x1`).
    Included,
    /// В блоке, откат (`status = 0x0`) — газ потрачен.
    Reverted,
    /// Не попала в блок за отведённое время (send.max_wait_blocks / deadline) — газ не потрачен.
    NotIncluded,
    /// Nonce занят другой транзакцией, нашей квитанции нет.
    Replaced,
}

/// Транзакция в сети: что отправили, куда, чем кончилось и сколько стоило.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainTx {
    /// Хеш — считается локально до отправки (keccak подписанных байт).
    pub hash: B256,
    pub nonce: u64,
    /// В какие каналы отправлена (id каналов).
    pub channels: Vec<String>,
    pub method: SendMethod,
    pub outcome: TxOutcome,
    /// Номер блока; None — не в блоке.
    pub block: Option<u64>,
    /// `gasUsed` из квитанции.
    pub gas_used: Option<u64>,
    /// `gasUsed × effectiveGasPrice` в BNB; None — квитанции нет.
    pub gas_bnb: Option<Decimal>,
    /// Чаевые билдеру в BNB (если были).
    pub tip_bnb: Option<Decimal>,
}
