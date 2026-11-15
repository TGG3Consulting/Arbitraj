//! Файл: core/types/dex_graph.rs
//! Что делает: решение 308 (задача t137) — состояние фонового владельца графа маршрутов DEX для экрана «Площадки»
//! (блок «Граф маршрутов (фон)»): режим, снимок (версия, возраст, отставание от реестра), последний накопительный круг,
//! последняя полная пересборка со сверкой, расхождения самопроверки, срабатывания предохранителя и ступени «снимок на
//! прежних настройках» за час и сутки, перезапуски. Только числа и тексты, посчитанные адаптером; ядро их не трогает.
//! Кто вызывает: venues/bsc_dex/pools/graph_stats.rs (заполняет), gateway/contract.rs (dex_graph_state), api/dex_graph.rs.
//! От чего зависит: serde, chrono.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Последний накопительный круг владельца.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DexGraphRound {
    pub at: DateTime<Utc>,
    /// Длительность круга, мс.
    pub ms: f64,
    /// Пулов оценено (изменённые и зависимые по курсу встречного токена).
    pub evaluated: usize,
    /// Пар, у которых пересобран топ (состав ребра).
    pub pairs: usize,
    /// Рёбер в опубликованном снимке.
    pub edges: usize,
    /// Отставание в момент публикации: возраст самой старой учтённой записи реестра, мс.
    pub lag_ms: u64,
    /// Сколько изменённых пулов уже ждут следующего круга в момент публикации.
    pub pending_after: usize,
}

/// Последняя полная пересборка (со сверкой).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DexGraphFull {
    pub at: DateTime<Utc>,
    /// Почему (запуск, настройки, переподключение лент, кнопка, срок самопроверки).
    pub reason: String,
    /// Длительность сборки с нуля и доводки, мс.
    pub ms: f64,
    /// Пулов в реестре на момент сборки.
    pub pools: usize,
    pub edges: usize,
    /// Было ли с чем сравнивать (накопительная карта существовала).
    pub compared: bool,
    /// Сколько отличий нашла сверка (0 — карты равны).
    pub differences: usize,
    /// Итог: «ok» или текст сбоя сборки.
    pub outcome: String,
}

/// Состояние фонового владельца графа.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DexGraphState {
    /// dex.graph_background = да.
    pub enabled: bool,
    /// Владелец запущен (подписка DEX на рынок есть).
    pub running: bool,
    /// Снимок опубликован.
    pub ready: bool,
    /// Почему снимка нет (не задана настройка графа и т. п.); None — нет причины или снимок есть.
    pub why_none: Option<String>,
    pub snapshot_version: Option<u64>,
    pub registry_version: u64,
    pub seq: Option<u64>,
    /// Снимок собран с нуля (иначе — накопительно).
    pub snapshot_full: Option<bool>,
    /// Возраст снимка, мс.
    pub age_ms: Option<u64>,
    /// Отставание снимка от реестра сейчас, мс (None — отставания нет).
    pub lag_ms: Option<u64>,
    /// Наибольшее отставание при публикации за последний час, мс.
    pub lag_max_hour_ms: Option<u64>,
    /// Изменённых пулов ждут круга.
    pub pending: usize,
    pub edges: Option<usize>,
    pub last_round: Option<DexGraphRound>,
    pub last_full: Option<DexGraphFull>,
    pub mismatches: u64,
    pub last_mismatch_at: Option<DateTime<Utc>>,
    pub last_mismatch: Option<String>,
    pub fuse_hour: u64,
    pub fuse_day: u64,
    pub stale_settings_hour: u64,
    pub stale_settings_day: u64,
    pub restarts: u64,
    pub last_restart: Option<String>,
}
