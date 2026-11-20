//! Файл: core/types/tip_share.rs
//! Что делает: решение 437 (3) — динамические чаевые сделок DEX ↔ DEX сети eip1559_l1 (Arbitrum, порядок секвенсора
//! PGA — по чаевым): состояние доли прибыли на чаевые по площадке DEX — текущая доля, последние чаевые, счётчики
//! (проигрыш по чаевым / по скорости / победы), последний проигрыш (наш хеш, перехватчик и его чаевые).
//! Кто вызывает: адаптер DEX (exec/tip_state.rs — ведёт и хранит), шлюз (контракт tip_share), api/tip_share.rs,
//! интерфейс (components/TipShare.svelte, экран «Каналы отправки»).
//! От чего зависит: chrono, rust_decimal, serde.
//! Настройки: send.tip_mode, send.tip_* [площадка DEX] (docs/SETTINGS.md).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Итог сделки цикла для доли чаевых.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TipOutcome {
    /// Наша транзакция в блоке без отката.
    Win,
    /// Нас опередила транзакция с чаевыми выше наших (в блоке включения, по пулам маршрута) — доля растёт.
    LostTip,
    /// Опередившая — с чаевыми не выше наших, или перехватчика нет: чаевыми не лечится, доля не меняется.
    LostSpeed,
}

/// Проигрыш: что нашли по блоку.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TipLoss {
    pub at: DateTime<Utc>,
    /// Наша транзакция (хеш).
    pub ours: String,
    /// Блок, где искали перехватчика (включения или ближайший после отправки — при отказе условия).
    pub block: u64,
    pub outcome: TipOutcome,
    /// Перехватчик с наибольшими чаевыми среди тех, кто изменил наш пул раньше нас; None — не найден.
    pub rival: Option<String>,
    /// Его чаевые: effectiveGasPrice − baseFee блока, gwei.
    pub rival_tip_gwei: Option<Decimal>,
    /// Наши чаевые, gwei.
    pub our_tip_gwei: Decimal,
}

/// Состояние доли чаевых площадки DEX.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TipShare {
    /// Площадка DEX.
    pub venue: String,
    /// send.tip_mode сейчас (None — не задана); причина, если режим не работает (настройки).
    pub mode: Option<String>,
    pub note: Option<String>,
    /// Текущая доля прибыли на чаевые, %; None — с начала или после сброса: берётся send.tip_share_min_pct.
    pub share_pct: Option<Decimal>,
    /// Чаевые последней сделки цикла, gwei.
    pub last_tip_gwei: Option<Decimal>,
    pub wins: u64,
    pub lost_tip: u64,
    pub lost_speed: u64,
    /// Побед подряд (для снижения доли).
    pub streak: u64,
    pub last_loss: Option<TipLoss>,
    pub updated_at: Option<DateTime<Utc>>,
}
