//! Файл: venues/bsc_dex/send/stats.rs
//! Что делает: счётчики каналов отправки (решение 42): отправлено (принято каналом), отклонено, в блоке, откатов,
//! газ впустую; последнее состояние канала. Каждое изменение — событие SendChannel ядру и интерфейсу.
//! Транзакция, отправленная в несколько каналов сразу, засчитывается каждому принявшему каналу: какой из них довёл
//! её до блока, по своему узлу не видно.
//! Кто вызывает: send/mod.rs, send/track.rs (итог), exec/*.
//! От чего зависит: core/types (SendChannelStats, ChannelState), gateway/events.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{ChannelState, SendChannelStats};
use crate::gateway::events::MarketEvent;

/// Счётчики всех каналов. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Stats {
    map: Arc<Mutex<HashMap<String, SendChannelStats>>>,
    out: Arc<Mutex<Option<mpsc::Sender<MarketEvent>>>>,
}

impl Stats {
    /// Куда слать события SendChannel (канал из subscribe_market). Ошибок нет.
    pub fn set_out(&self, out: mpsc::Sender<MarketEvent>) {
        *self.out.lock().unwrap_or_else(|p| p.into_inner()) = Some(out);
    }

    /// Счётчики канала (пустые, если не было). Ошибок нет.
    pub fn get(&self, id: &str, state: ChannelState) -> SendChannelStats {
        let m = self.map.lock().unwrap_or_else(|p| p.into_inner());
        m.get(id).cloned().unwrap_or(SendChannelStats {
            id: id.to_string(),
            state,
            sent: 0,
            rejected: 0,
            included: 0,
            reverted: 0,
            gas_wasted_bnb: Decimal::ZERO,
            updated_at: Utc::now(),
        })
    }

    /// Изменяет счётчики канала и шлёт событие. Принимает: id, функцию изменения. Ошибок нет.
    pub fn update(&self, id: &str, f: impl FnOnce(&mut SendChannelStats)) {
        let snapshot = {
            let mut m = self.map.lock().unwrap_or_else(|p| p.into_inner());
            let s = m.entry(id.to_string()).or_insert_with(|| SendChannelStats {
                id: id.to_string(),
                state: ChannelState::Ready,
                sent: 0,
                rejected: 0,
                included: 0,
                reverted: 0,
                gas_wasted_bnb: Decimal::ZERO,
                updated_at: Utc::now(),
            });
            f(s);
            s.updated_at = Utc::now();
            s.clone()
        };
        if let Some(out) = self.out.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            let _ = out.try_send(MarketEvent::SendChannel(snapshot));
        }
    }

    /// Событие ядру (например очередь кошелька) без ожидания: нет канала — ничего. Ошибок нет.
    pub fn emit(&self, ev: MarketEvent) {
        if let Some(out) = self.out.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            let _ = out.try_send(ev);
        }
    }
}
