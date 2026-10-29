//! Файл: core/types/cost.rs
//! Что делает: затраты сделки (вид, сумма, в чём, шаг маршрута) и маршрут обмена на DEX (прямой пул или умный).
//! Кто вызывает: адаптеры (котировка, стоимость вывода), ядро (полный список затрат, решение 35),
//! интерфейс (расчёт кандидата).
//! От чего зависит: core/types/pool.rs, rust_decimal.
//! Настройки: нет.
//!
//! Правило: адаптер кладёт в котировку всё, что знает о своей стороне сделки; ядро добавляет своё
//! (вывод, возврат USDT, запас на движение цены) и переводит всё в USDT по живым ценам.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::pool::{PoolKind, PoolRef};

/// Вид затраты. Список — решение 35 и CLAUDE.md («все затраты»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CostKind {
    /// Торговая комиссия биржи (в той валюте, что списывает биржа).
    TradeFee,
    /// Комиссия пула (V2 — из настройки; V3 — fee(); V4 — текущая lp+protocol).
    PoolFee,
    /// Остаток от округления количества по шагу биржи.
    StepRemainder,
    /// Газ обмена (BNB).
    Gas,
    /// Газ разрешения approve/Permit2 — только в первый раз на монету (BNB).
    ApproveGas,
    /// Газ перевода монеты из кошелька на биржу (BNB).
    TransferGas,
    /// Газ обёртки нативной монеты в транзакции обмена (решение 62): WRAP_ETH при продаже BNB на DEX, UNWRAP_WETH
    /// при покупке (BNB) — сверх газа того же обмена токеном WBNB, измерено пробным обменом.
    WrapGas,
    /// Налог токена при покупке.
    BuyTax,
    /// Налог токена при продаже.
    SellTax,
    /// Налог токена при переводе.
    TransferTax,
    /// Влияние крючка группы B/C, измеренное пробным обменом.
    HookImpact,
    /// Чаевые каналу отправки (BNB).
    ChannelTip,
    /// Комиссия вывода биржи.
    WithdrawFee,
    /// Стоимость перегона USDT обратно (режим «в каждой сделке», решение 34) — считает ядро.
    UsdtReturn,
    /// Запас на движение цены за время перевода r = k·σ·√T — считает ядро.
    PriceMoveReserve,
}

/// Одна затрата.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub kind: CostKind,
    /// Сумма в единицах `asset`, неотрицательная.
    pub amount: Decimal,
    /// В чём: «USDT», «BNB» или тикер монеты.
    pub asset: String,
    /// true — уже учтено в средней цене котировки (показывается для прозрачности, повторно не вычитается);
    /// false — вычитается отдельно.
    pub in_price: bool,
    /// Шаг маршрута обмена, к которому относится затрата (решение 52): комиссия пула, проскальзывание,
    /// газ, налог — на каждом шаге «USDT → BNB», «BNB → монета». None — затрата всего маршрута или не обмена
    /// (комиссия биржи, вывод, возврат USDT, запас r). Прямой одношаговый маршрут — шаг 1.
    #[serde(default)]
    pub step: Option<RouteStep>,
}

/// Шаг маршрута обмена: номер (с 1) и пара токенов «откуда → куда» (решение 52).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RouteStep {
    /// Номер шага по порядку, с 1.
    pub index: u32,
    /// Тикер того, что отдаём на шаге (например «USDT»).
    pub from: String,
    /// Тикер того, что получаем на шаге (например «BNB»).
    pub to: String,
    /// Адреса токенов в BSC (адрес — главный идентификатор); None — нативная монета или неизвестно.
    pub from_token: Option<alloy_primitives::Address>,
    pub to_token: Option<alloy_primitives::Address>,
}

impl Cost {
    /// Создаёт затрату без шага маршрута. Принимает: вид, сумму, актив, учтена ли в цене. Ошибок нет.
    pub fn new(kind: CostKind, amount: Decimal, asset: impl Into<String>, in_price: bool) -> Self {
        Self {
            kind,
            amount,
            asset: asset.into(),
            in_price,
            step: None,
        }
    }

    /// Привязывает затрату к шагу маршрута (адаптер DEX — для каждого обмена маршрута).
    /// Принимает: шаг. Возвращает: ту же затрату с шагом.
    pub fn with_step(mut self, step: RouteStep) -> Self {
        self.step = Some(step);
        self
    }
}

/// Режим обмена на DEX (решения 29, 38, 80).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteMode {
    /// Лучший одиночный пул, прямой контракт обмена.
    Direct,
    /// Маршрут через несколько пулов (хопы, доли), исполняет официальный Universal Router.
    Smart,
    /// Маршрут по локальному графу токенов (решение 80): любые площадки и промежуточные монеты, сплит между пулами.
    /// Исполнимость одной транзакцией — поле DexRoute.executable (решение 84).
    Graph,
}

/// Один пул на пути доли.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteHop {
    /// Вид пула у адаптера (id области «DEX»).
    pub family: String,
    pub kind: PoolKind,
    pub pool: PoolRef,
}

/// Доля маршрута: часть входа идёт по цепочке пулов.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteLeg {
    /// Доля входа, 0–1. У прямого обмена — одна доля 1.
    pub share: Decimal,
    pub hops: Vec<RouteHop>,
    /// Ожидаемый выход доли по расчёту адаптера (в единицах выходного актива).
    pub expected_out: Decimal,
}

/// Маршрут обмена на DEX, на который рассчитана котировка; тот же маршрут уходит в TradeRequest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DexRoute {
    pub mode: RouteMode,
    pub legs: Vec<RouteLeg>,
    /// Блок, по состоянию которого посчитан маршрут.
    pub block: u64,
    /// Оценка газа обмена (единиц газа).
    pub gas_units: u64,
    /// Исполним ли маршрут одной транзакцией как есть (решение 84): прямая форма (один пул) или всё через один
    /// Universal Router (умный путь PancakeSwap). false — маршрут выходит за один роутер и нужен бандл свопов
    /// (решение 84, фаза исполнения): это сигнал/карточка, авто-сделкой пока не исполняется. Direct/Smart — всегда true.
    #[serde(default = "executable_default")]
    pub executable: bool,
    /// Причина неисполнимости как есть (для карточки сигнала, решение 74). None — исполним.
    #[serde(default)]
    pub exec_note: Option<String>,
}

/// По умолчанию маршрут исполним (старые записи без поля, Direct/Smart). Ошибок нет.
fn executable_default() -> bool {
    true
}
