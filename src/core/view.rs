//! Файл: core/view.rs
//! Что делает: живой поток ядра для интерфейса — события в формате `{"type": …, "data": …}`
//! (src/api/README.md: coin_check, pool_check, coin_block, stuck_alert, manual, send_channel, gas_stats)
//! и дополнительные: spread (строка «Живого экрана»), coins (справочник изменился — перечитать GET),
//! candidate, deal, balances, control (появляются в следующих частях ядра).
//! Кто вызывает: модули ядра (публикуют), api/ws.rs (подписывается через Core::subscribe); api/core_api.rs —
//! Core::candidates (последние расчёты кандидатов) и Core::channel_stats.
//! От чего зависит: core/types, signal.rs, serde.
//! Настройки: нет.

use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{Notify, broadcast};

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
    /// Сводка эпизода арбитражной ситуации (решение 74): открыт, запись, закрыт.
    Episode(Value),
    /// Сводка окна (решение 125): открыто, вошло направление, закрыто.
    Window(Value),
    /// Итог команды человека (решение 156): {action, who, ok, text} — Telegram правит сообщения записи.
    Command(Value),
}

/// Отправитель живого потока ядра. Дешёво клонируется.
#[derive(Clone)]
pub struct Live {
    tx: broadcast::Sender<CoreEvent>,
    // Отдельное кольцо итогов кнопок: новые цены не вытесняют итог ручной операции.
    commands: broadcast::Sender<Value>,
    mirror_dirty: Arc<Notify>,
}

impl Live {
    /// Создаёт поток. Принимает: размер буфера (--live-buffer).
    pub fn new(buffer: usize) -> Self {
        let (tx, _) = broadcast::channel(buffer);
        let (commands, _) = broadcast::channel(buffer);
        Self {
            tx,
            commands,
            mirror_dirty: Arc::new(Notify::new()),
        }
    }

    /// Публикует событие. Нет слушателей — событие просто не нужно никому; ошибок нет.
    pub fn send(&self, ev: CoreEvent) {
        if self.commands.receiver_count() != 0 {
            if let CoreEvent::Command(value) = &ev {
                let _ = self.commands.send(value.clone());
            } else {
                // Единственный дубль перечитывает текущее состояние всех карточек. Одного
                // ожидающего пробуждения достаточно для всех событий, пришедших за время отправки.
                self.mirror_dirty.notify_one();
            }
        }
        let _ = self.tx.send(ev);
    }

    /// Подписка единственного Telegram-дубля: итоги команд по порядку, остальные изменения
    /// будят сверку актуального состояния экрана. После запуска первая сверка выполняется сразу.
    pub fn subscribe_mirror(&self) -> (broadcast::Receiver<Value>, Arc<Notify>) {
        (self.commands.subscribe(), self.mirror_dirty.clone())
    }

    /// Подписка для интерфейса (api/ws.rs). Отстал — получит Lagged и перечитает данные по REST.
    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.tx.subscribe()
    }
}

/// Строки «Живого экрана» и экрана «Каналы отправки» (перенесены из candidates.rs, решение 129 — размер файла).
impl super::engine::Core {
    /// Последние расчёты кандидатов по направлениям (новые сверху) — «Живой экран».
    pub fn candidates(&self) -> serde_json::Value {
        let mut v: Vec<serde_json::Value> = super::engine::lk(&self.inner.candidates)
            .values()
            .cloned()
            .collect();
        v.sort_by(|a, b| b["at"].as_str().cmp(&a["at"].as_str()));
        serde_json::Value::Array(v)
    }

    /// Последняя статистика каналов отправки (события адаптеров DEX) — экран «Каналы отправки».
    pub fn channel_stats(&self) -> Vec<crate::core::types::SendChannelStats> {
        super::engine::rd(&self.inner.market).channels()
    }
}
