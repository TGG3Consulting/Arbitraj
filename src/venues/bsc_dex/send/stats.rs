//! Файл: venues/bsc_dex/send/stats.rs
//! Что делает: счётчики каналов отправки (решение 42): отправлено (принято каналом), отклонено, в блоке, откатов,
//! газ впустую; последнее состояние канала. Каждое изменение — событие SendChannel ядру и интерфейсу.
//! Транзакция, отправленная в несколько каналов сразу, засчитывается каждому принявшему каналу: какой из них довёл
//! её до блока, по своему узлу не видно.
//! Решение 333 (задача t155): события ядру (SendChannel и emit — WalletQueue, ExitGap, Notice исполнения) идут через
//! gateway/core_out_must.rs, а не `let _ = out.try_send`: при полном канале ядра WalletQueue и ExitGap (по ним ядро решает
//! о деньгах) ждут места по порядку при venue.money_events_keep = да, Notice и SendChannel — при venue.info_events_keep =
//! да; не «да» (не заданы) — как раньше потеряны, но с записью журнала venue.core_events_lost (счёт).
//! Кто вызывает: send/mod.rs, send/track.rs (итог), exec/*; parts.rs (set_out).
//! От чего зависит: core/types (SendChannelStats, ChannelState), gateway/events, gateway/core_out_must.rs.
//! Настройки: свои — нет; через core_out_must.rs — venue.money_events_keep, venue.info_events_keep,
//! log.events_lost_interval_s.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{ChannelState, SendChannelStats};
use crate::gateway::core_out_must::MustOut;
use crate::gateway::events::MarketEvent;

/// Счётчики всех каналов. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Stats {
    map: Arc<Mutex<HashMap<String, SendChannelStats>>>,
    out: Arc<Mutex<Option<MustOut>>>,
}

impl Stats {
    /// Куда слать события SendChannel и emit (канал из subscribe_market; настройки и журнал — решение 333). Ошибок нет.
    pub fn set_out(&self, out: MustOut) {
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
            // Решение 333: полный канал — ждёт места (venue.info_events_keep) или потеря со счётом в журнале.
            out.send(MarketEvent::SendChannel(snapshot));
        }
    }

    /// Событие ядру (очередь кошелька, замер выхода, уведомление) без ожидания; полный канал — решение 333
    /// (gateway/core_out_must.rs: WalletQueue и ExitGap ждут места при venue.money_events_keep = да, остальное — при
    /// venue.info_events_keep = да; иначе потеря со счётом в журнале). Нет канала — ничего. Ошибок нет.
    pub fn emit(&self, ev: MarketEvent) {
        if let Some(out) = self.out.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            out.send(ev);
        }
    }
}
