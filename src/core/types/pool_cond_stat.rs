//! Файл: core/types/pool_cond_stat.rs
//! Что делает: решение 439 — статистика сделок цикла DEX ↔ DEX через исполнитель площадки DEX сети eip1559_l1
//! (Arbitrum) по режиму условия пулов send.pool_cond_mode на момент отправки (off / pools): отправки, победы, откаты и
//! оплаченный ими газ, бесплатные отказы по условию пулов и по окну времени. По ней Тигран решает, включать ли режим
//! pools (решение Тиграна 20.11.2026).
//! Кто вызывает: адаптер DEX (exec/pool_cond_stat.rs — ведёт и хранит), шлюз (контракт pool_cond_stat),
//! api/pool_cond_stat.rs, интерфейс (components/PoolCondStat.svelte, экран «Каналы отправки»).
//! От чего зависит: chrono, rust_decimal, serde.
//! Настройки: send.pool_cond_mode [площадка DEX] (docs/SETTINGS.md).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Счётчики одного режима. Ревью 439 (6): serde(default) — запись SQLite без нового поля разбирается (поле — 0).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolCondCounts {
    /// Итогов отправки (каждый вызов отправки транзакции цикла, любой итог — в том числе сбой отправки и «исход
    /// неизвестен»; прочие итоги = отправки − победы − откаты − отказы).
    pub sends: u64,
    /// В блоке без отката.
    pub wins: u64,
    /// Откаты (в блоке, статус 0).
    pub reverts: u64,
    /// Газ откатов в нативной монете сети: сумма effectiveGasPrice × gasUsed из квитанций.
    pub revert_gas: Decimal,
    /// Откаты, газ которых не учтён (квитанции нет или число не помещается).
    pub revert_gas_unknown: u64,
    /// Бесплатные отказы −32003 по условию пулов («Storage … condition not met»).
    pub rejected_pools: u64,
    /// Отказы −32003 по окну времени (timestampMax).
    pub rejected_time: u64,
}

/// Статистика площадки DEX. Ревью 439 (6): serde(default) — запись SQLite без нового поля разбирается.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PoolCondStat {
    /// Площадка DEX.
    pub venue: String,
    /// send.pool_cond_mode сейчас: off / pools (не задана — off); причина, если значение не разобрано.
    pub mode: Option<String>,
    pub note: Option<String>,
    /// Ревью 438 (E): режим pools при нескольких включённых каналах — условие уйдёт только в канал sequencer.
    pub channels_note: Option<String>,
    /// Режим off (и настройка не задана) на момент отправки.
    pub off: PoolCondCounts,
    /// Режим pools на момент отправки.
    pub pools: PoolCondCounts,
    /// С какого момента считается (последний сброс; None — с первой сделки).
    pub since: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}
