//! Файл: core/view.rs
//! Что делает: живой поток ядра для интерфейса — события в формате `{"type": …, "data": …}`
//! (src/api/README.md: coin_check, pool_check, coin_block, stuck_alert, manual, send_channel, gas_stats)
//! и дополнительные: spread (строка «Живого экрана»), coins (справочник изменился — перечитать GET),
//! candidate, deal, balances, control (появляются в следующих частях ядра).
//! Кто вызывает: модули ядра (публикуют), api/ws.rs (подписывается через Core::subscribe).
//! От чего зависит: core/types, signal.rs, serde.
//! Настройки: нет.

use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::core::types::{
    Asset, CoinBlock, CoinCheck, CoinGasStats, ManualHolding, PoolCheck, SendChannelStats,
    StuckAlert, WalletQueue,
};

use super::signal::SpreadView;

/// Событие ядра для интерфейса.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum CoreEvent {
    CoinCheck(CoinCheck),
    PoolCheck(PoolCheck),
    /// block = null — монета вышла из чёрного списка.
    CoinBlock {
        asset: Asset,
        block: Option<CoinBlock>,
    },
    StuckAlert(StuckAlert),
    Manual {
        alerts: Vec<StuckAlert>,
        holdings: Vec<ManualHolding>,
    },
    SendChannel(SendChannelStats),
    /// Очередь кошелька DEX (свободна / ждёт итога / остановлена до ручной проверки).
    WalletQueue(WalletQueue),
    GasStats(CoinGasStats),
    /// Строка «Живого экрана»: спред направления и его состояние.
    Spread(SpreadView),
    /// Справочник монет пересобран: сколько монет и направлений; список — GET /api/coins.
    Coins {
        coins: usize,
        directions: usize,
    },
    /// Произвольные данные следующих частей ядра (расчёт кандидата, сделка, балансы, управление).
    Candidate(Value),
    Deal(Value),
    Balances(Value),
    Control(Value),
}

/// Отправитель живого потока ядра. Дешёво клонируется.
#[derive(Clone)]
pub struct Live {
    tx: broadcast::Sender<CoreEvent>,
}

impl Live {
    /// Создаёт поток. Принимает: размер буфера (--live-buffer).
    pub fn new(buffer: usize) -> Self {
        let (tx, _) = broadcast::channel(buffer);
        Self { tx }
    }

    /// Публикует событие. Нет слушателей — событие просто не нужно никому; ошибок нет.
    pub fn send(&self, ev: CoreEvent) {
        let _ = self.tx.send(ev);
    }

    /// Подписка для интерфейса (api/ws.rs). Отстал — получит Lagged и перечитает данные по REST.
    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.tx.subscribe()
    }
}
