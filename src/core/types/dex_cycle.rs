//! Файл: core/types/dex_cycle.rs
//! Что делает: тип «сигнал замкнутого цикла DEX ↔ DEX» (решение 85, фаза сигнала) для «Живого экрана» — по образцу
//! карточки сигнала (решение 74): базовая монета (старт = финиш), вложено и получено базовой монеты, валовая и чистая
//! прибыль, затраты газа и чаевых (BNB) и их перевод в базовую монету, маршрут цикла по хопам (через какой пул и на
//! каких условиях), флаг исполнимости и причина. Это read-данные ядра: ничего не исполняет, авто-сделки не запускает.
//! Чистую прибыль показываем, только если газ/чаевые удалось перевести в базовую монету (иначе net_profit = None —
//! видна валовая, ничего не скрыто: «не торгуем вслепую»). Адаптер DEX заполняет этот тип из своего CycleSignal.
//! Кто вызывает: адаптер DEX (заполняет — Venue::dex_cycles), api/dex.rs (отдаёт GET /api/venues/{id}/dex-cycles), интерфейс.
//! От чего зависит: core/types/market.rs, core/types/pool.rs, alloy-primitives, rust_decimal.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

use alloy_primitives::Address;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::VenueId;
use super::pool::{PoolKind, PoolRef};

/// Один хоп цикла для карточки (решение 74/85): через какой пул и на каких условиях меняется токен.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DexCycleHop {
    /// Номер шага по порядку, с 1.
    pub index: u32,
    /// Вид пула у адаптера (id семейства DEX).
    pub family: String,
    pub kind: PoolKind,
    pub pool: PoolRef,
    /// Токен входа этого хопа (адрес в BSC).
    pub token_in: Address,
    /// Токен выхода этого хопа (адрес в BSC).
    pub token_out: Address,
    /// Комиссия пула на этом шаге, % (уже в выходе).
    pub fee_pct: Decimal,
    /// Влияние крючка B/C на этом шаге, % (уже в выходе).
    pub hook_pct: Decimal,
    /// Газ обмена в этом пуле (единиц).
    pub gas_units: u64,
}

/// Сигнал одного замкнутого цикла DEX ↔ DEX (решение 85). Чистая прибыль посчитана, только если газ/чаевые (BNB)
/// удалось перевести в базовую монету; иначе net_profit = None и видна валовая прибыль (ничего не скрыто).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DexCycle {
    /// Площадка DEX, на которой найден цикл.
    pub venue: VenueId,
    /// Базовая монета (старт = финиш цикла), адрес в BSC.
    pub base: Address,
    /// Тикер базовой монеты.
    pub base_ticker: String,
    /// Вложено базовой монеты (вход цикла).
    pub input: Decimal,
    /// Получено базовой монеты (выход цикла; комиссии пулов, крючки, проскальзывание уже внутри).
    pub output: Decimal,
    /// Валовая прибыль базовой монеты = выход − вход.
    pub gross_profit: Decimal,
    /// Единиц газа на все свопы цикла.
    pub gas_units: u64,
    /// Газ свопов цикла, BNB.
    pub gas_bnb: Decimal,
    /// Чаевые канала отправки, BNB (решение 41; из настроек, 0 — не заданы).
    pub tip_bnb: Decimal,
    /// Затраты BNB всего (газ + чаевые); газ разрешений (approve) сюда НЕ входит (фаза исполнения).
    pub bnb_cost: Decimal,
    /// Затраты BNB, переведённые в базовую монету (None — перевести нечем: нет пула базовая ↔ WBNB).
    pub bnb_cost_in_base: Option<Decimal>,
    /// Чистая прибыль базовой монеты = валовая − bnb_cost_in_base (None — газ/чаевые не переведены в базовую).
    pub net_profit: Option<Decimal>,
    /// Маршрут цикла по шагам.
    pub hops: Vec<DexCycleHop>,
    /// Цикл исполним как есть (все хопы через tradable-пулы, решение 84/85). Авто-сделки этот сигнал НЕ запускает.
    pub executable: bool,
    /// Почему не исполним (для карточки). None — исполним.
    pub exec_note: Option<String>,
    /// Блок, по состоянию которого посчитан цикл (минимальный среди пулов маршрута).
    pub block: u64,
}
