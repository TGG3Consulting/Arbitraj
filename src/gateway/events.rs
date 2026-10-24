//! Файл: gateway/events.rs
//! Что делает: события, которые адаптеры шлют в шлюз, а шлюз — ядру и интерфейсу.
//! Кто вызывает: адаптеры (события адаптера — в канал из subscribe_market), шлюз (подключение и ошибки).
//! От чего зависит: core/types, gateway/errors.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::types::{
    CoinCheck, OrderBook, Pool, PoolCheck, SendChannelStats, VenueId, WalletQueue,
};

use super::errors::GatewayError;

/// Событие от адаптера в канал, переданный в subscribe_market: рынок (стакан, пул) и то,
/// что решает, можно ли торговать монетой (проверка налогов, крючков, состояние каналов отправки).
/// Ядро обновляет своё состояние и передаёт всё в интерфейс (api/ws.rs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarketEvent {
    /// Полный актуальный стакан монеты (адаптер сам собирает его из снимка и обновлений).
    Book(OrderBook),
    /// Актуальное состояние пула DEX.
    Pool(Pool),
    /// Площадка сообщила, что данные по монете больше недостоверны
    /// (разрыв последовательности, переподключение). Ядро считает данные устаревшими.
    Invalidated { venue: VenueId, ticker: String },
    /// DEX: проверка налогов монеты закончилась или изменилась (решение 30).
    /// NotChecked — проверка началась, торговать нельзя.
    CoinCheck(CoinCheck),
    /// DEX: проверка влияния крючка пула группы B/C (решение 32).
    PoolCheck(PoolCheck),
    /// DEX: состояние и счётчики канала отправки изменились (решения 41–42).
    SendChannel(SendChannelStats),
    /// DEX: состояние очереди транзакций кошелька изменилось (остановлена до ручной проверки, снята, занята).
    WalletQueue(WalletQueue),
    /// DEX: полный проход проверки налогов закончен (первый после подписки — ядро шлёт одно сводное coin.check_done).
    CheckPass {
        venue: VenueId,
        /// Сколько монет: налоги известны, проверка не прошла, ждут достоверного пула, не задана настройка.
        ok: usize,
        failed: usize,
        waiting: usize,
        not_configured: usize,
    },
    /// Сообщение адаптера для уведомлений: код события из notify/catalog.rs и текст (ядро шлёт, если событие включено).
    Notice {
        venue: VenueId,
        code: String,
        text: String,
    },
}

/// Состояние подключения площадки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Connected,
    Reconnecting,
    Disconnected,
}

/// Служебное событие шлюза: подключение или ошибка.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GatewayEvent {
    Connection {
        venue: VenueId,
        state: ConnectionState,
        at: DateTime<Utc>,
    },
    Error {
        error: GatewayError,
        at: DateTime<Utc>,
    },
}
