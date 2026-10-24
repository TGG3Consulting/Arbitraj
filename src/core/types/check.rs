//! Файл: core/types/check.rs
//! Что делает: результаты проверки монет и пулов — налоги токена (покупка, продажа, перевод),
//! влияние крючка V4, состояние проверки (не проверено / известно / ошибка с причиной)
//! и запись чёрного списка с причиной.
//! Кто вызывает: адаптер DEX (проверяет и шлёт события), ядро (справочник, чёрный список, расчёт),
//! интерфейс (экран «Монеты»), уведомления.
//! От чего зависит: core/types/market.rs, core/types/pool.rs.
//! Настройки: пороги — tax.*, hooks.* (их применяет ядро и адаптер, не эти типы).
//!
//! Решения 30 и 32: пока проверка не закончена — торговли нет; ошибка проверки — чёрный список с причиной;
//! перепроверка по циклу; прошедшая монета выходит из списка сама; ручной чёрный список (решение 36).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::{Asset, VenueId};
use super::pool::{HookGroup, PoolRef};

/// Состояние проверки чего-либо (налога монеты, влияния крючка).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CheckState<T> {
    /// Ещё не проверено — торговать нельзя.
    NotChecked,
    /// Проверено, значение известно.
    Known(T),
    /// Проверка не прошла — причина как есть (ответ узла, откат пробного обмена и т.п.).
    Failed { reason: String },
}

/// Налоги токена в процентах от суммы (0–100).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenTaxes {
    /// Налог при покупке (получили меньше монет, чем отдал пул).
    pub buy_pct: Decimal,
    /// Налог при продаже (пул получил меньше монет, чем мы отправили).
    pub sell_pct: Decimal,
    /// Налог при простом переводе (кошелёк → биржа).
    pub transfer_pct: Decimal,
}

/// Вид налога — для причины чёрного списка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaxKind {
    Buy,
    Sell,
    Transfer,
}

/// Результат проверки налогов монеты на площадке DEX.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoinCheck {
    pub venue: VenueId,
    pub asset: Asset,
    pub taxes: CheckState<TokenTaxes>,
    /// Источник проверки (архитектура подключаемая, решение 30): например «eth_call» — свой пробный обмен.
    pub source: String,
    /// Когда проверено; None — ещё не проверялось.
    pub checked_at: Option<DateTime<Utc>>,
    /// Почему проверка ещё не выполнена (taxes = NotChecked): «нет пула ≥ dex.min_pool_liquidity_usd», «нет пула-источника
    /// USDT для пробы», «узел: …», «не задана настройка …». None — не ждёт или причина не известна.
    #[serde(default)]
    pub wait: Option<String>,
}

/// Влияние крючка на обмен, измеренное пробным обменом (решение 32), в процентах от суммы.
/// Положительное — получаем меньше, чем по математике пула без крючка.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookImpact {
    pub buy_pct: Decimal,
    pub sell_pct: Decimal,
}

/// Результат проверки пула с крючком группы B или C.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolCheck {
    pub venue: VenueId,
    /// Вид пула у адаптера (id области «DEX»).
    pub family: String,
    pub pool: PoolRef,
    /// Монета пула (вторая валюта — USDT).
    pub asset: Asset,
    pub group: HookGroup,
    pub impact: CheckState<HookImpact>,
    pub checked_at: Option<DateTime<Utc>>,
}

/// Почему монета в чёрном списке.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BlockReason {
    /// Проверка налогов ещё не закончена.
    NotChecked,
    /// Проверка не прошла — причина от адаптера.
    CheckFailed { reason: String },
    /// Налог выше порога настройки tax.max_*_pct.
    TaxAboveLimit {
        tax: TaxKind,
        pct: Decimal,
        limit: Decimal,
    },
    /// Ручной чёрный список из интерфейса (настройки coin.blocked, coin.blocked_note).
    Manual { note: Option<String> },
}

/// Запись чёрного списка монеты.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoinBlock {
    pub asset: Asset,
    pub reason: BlockReason,
    pub since: DateTime<Utc>,
}
