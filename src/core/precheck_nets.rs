//! Файл: core/precheck_nets.rs
//! Что делает: решение 266 (Тигран 13.11.2026), часть 3 — справочник сетей на сделке только когда память старая
//! (precheck.networks_memory = да). Предпроверка (precheck.rs transfer_open) на каждую биржевую сторону делала HTTP
//! coin_networks_of; фон (registry_sync.rs) и так держит тот же справочник в памяти ядра (core.inner.networks) с
//! интервалом venue.networks_poll_ms (не задана — venue.status_poll_ms). Здесь — выбор: записи монеты этой площадки в
//! памяти моложе интервала фонового опроса ЭТОЙ площадки (тот же интервал и то же правило выбора настройки —
//! registry_sync.rs poll_ms) — HTTP не делается, проверка «вывод/ввод открыт» идёт по записям из памяти тем же кодом и
//! с теми же текстами отказов. Запрос как раньше — если: настройка не «да»; интервал не задан; записей монеты (по
//! тикеру площадки) в памяти нет; среди них нет сети с адресом контракта монеты направления (отказ «нет сети BSC с тем
//! же адресом» из памяти не выдаётся — только после запроса); хотя бы одна запись старше интервала или её время в
//! будущем (возраст определить нельзя). Возраст — по времени записи CoinNetworkInfo::updated_at: его ставит адаптер при
//! разборе ответа биржи (и у фонового опроса, и у запроса предпроверки); берётся самая старая запись монеты.
//! Статусы в памяти от фонового опроса — после сглаживания мигания (status_debounce.rs, venue.status_confirm_polls):
//! смена статуса, ещё не подтверждённая нужным числом опросов, в памяти не видна, а запрос предпроверки её видел.
//! Кто вызывает: core/precheck.rs (transfer_open).
//! От чего зависит: core/registry_sync.rs (poll_ms), engine.rs (справочник сетей ядра), core/types.
//! Настройки: precheck.networks_memory (читает executor.rs через precheck_fast.rs read), venue.networks_poll_ms,
//! venue.status_poll_ms (через registry_sync.rs).

use alloy_primitives::Address;
use chrono::Utc;

use crate::core::types::{CoinNetworkInfo, VenueId};

use super::engine::{Core, rd};

/// Включатель части 3.
pub const MEMORY: &str = "precheck.networks_memory";

/// Откуда взят справочник сетей шага предпроверки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    /// Запрос к бирже (как до решения 266).
    Http,
    /// Из памяти ядра; возраст самой старой записи монеты, мс.
    Memory(i64),
}

impl Src {
    /// Текст источника для подробного замера («Время по шагам (предпроверка)»). Ошибок нет.
    pub fn text(&self) -> String {
        match self {
            Src::Http => "запрос".into(),
            Src::Memory(ms) => format!("из памяти, возраст {ms} мс"),
        }
    }
}

/// Что делает: записи монеты площадки из памяти ядра, если они моложе интервала фонового опроса этой площадки.
/// Принимает: ядро, биржу, тикер монеты на этой бирже, адрес контракта монеты направления. Возвращает: Some(записи,
/// возраст самой старой, мс) — запрос к бирже не нужен; None — нужен запрос (причины — в шапке). Ошибок нет.
pub fn recent(
    core: &Core,
    v: &VenueId,
    on: &str,
    contract: Option<Address>,
) -> Option<(Vec<CoinNetworkInfo>, i64)> {
    let every = super::registry_sync::poll_ms(&core.inner.deps.settings, v)?;
    let every = i64::try_from(every).ok()?;
    let list: Vec<CoinNetworkInfo> = rd(&core.inner.networks)
        .get(v)?
        .iter()
        .filter(|i| i.asset.ticker == on)
        .cloned()
        .collect();
    let contract = contract?;
    if !list.iter().any(|i| i.asset.bsc_contract == Some(contract)) {
        return None;
    }
    let now = Utc::now();
    let ages = list.iter().map(|i| (now - i.updated_at).num_milliseconds());
    let (young, old) = ages.fold((i64::MAX, i64::MIN), |(a, b), x| (a.min(x), b.max(x)));
    (young >= 0 && old < every).then_some((list, old))
}
