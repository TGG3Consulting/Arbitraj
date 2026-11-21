//! Файл: core/types/cycle_trade.rs
//! Что делает: типы ИСПОЛНЕНИЯ цикла DEX ↔ DEX (решение 124; направление — решение 85): расчёт цикла на заданный вход
//! (для подбора размера ядром — core/cycle_plan.rs), запрос сделки цикла (вход и граница minOut ядра) и её итог (факт
//! свопа и курсы для учёта в USDT). Сам цикл — DexCycle (сигнал, core/types/dex_cycle.rs): маршрут по пулам; адаптер
//! пересобирает его по своему реестру и свежему состоянию пулов.
//! Кто вызывает: ядро (core/cycle_plan.rs, core/cycle_deal.rs, core/cycle_watch.rs), адаптер DEX (Venue::dex_cycle_quote,
//! Venue::dex_cycle_trade, Venue::dex_dry_cycle), api/dex_dry.rs.
//! От чего зависит: core/types/{dex_cycle, trade}.rs, rust_decimal, serde.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::dex_cycle::DexCycle;
use super::market::VenueId;
use super::trade::TradeResult;

/// Расчёт цикла на один вход (базовая монета) по состоянию пулов в памяти адаптера. Все суммы — в базовой монете,
/// затраты BNB — отдельно и переведённые в базовую (курс из графа, как у сигнала).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleQuote {
    /// Вход цикла (базовая монета).
    pub input: Decimal,
    /// Ожидаемый выход (комиссии пулов, крючки, проскальзывание уже внутри).
    pub output: Decimal,
    /// Затраты в BNB: газ свопов + газ разрешения (если его нет у кошелька) + чаевые канала.
    pub cost_bnb: Decimal,
    /// Те же затраты в базовой монете.
    pub cost_base: Decimal,
    /// Чистая прибыль в базовой = выход − вход − cost_base.
    pub net_base: Decimal,
    /// Курс: USDT за 1 базовой (базовая USDT — 1).
    pub base_usdt: Decimal,
    /// Курс: USDT за 1 BNB (газ в учёте сделки).
    pub bnb_usdt: Decimal,
    /// Свободно базовой монеты у кошелька — только если просили (with_wallet), иначе None.
    pub wallet_base: Option<Decimal>,
    /// Форма исполнения: «исполнитель 0x…» или «бандл свопов».
    pub form: String,
}

/// Запрос сделки цикла: вход и граница выхода, посчитанные ядром (core/cycle_plan.rs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleTradeRequest {
    pub venue: VenueId,
    /// Идентификатор сделки ядра (журнал).
    pub deal_id: String,
    /// Цикл из сигнала (маршрут по пулам, базовая монета).
    pub cycle: DexCycle,
    /// Вход цикла, базовая монета.
    pub input: Decimal,
    /// Наименьший допустимый выход, базовая монета: вход + затраты + порог прибыли (не ниже ожидаемого × (1 − допуск)).
    /// У исполнителя ArbExecutor — его minOut (контракт откатывает транзакцию, если вернулось меньше).
    pub min_out: Decimal,
}

/// Итог сделки цикла.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleTradeResult {
    /// Своп «базовая → базовая» в формате ядра: сторона Sell, qty = вход, avg_price = выход / вход, fee = газ BNB,
    /// chain — транзакция (бандл — сводно), executor — кто исполнил.
    pub trade: TradeResult,
    /// Вход и выход по факту (события Transfer базовой монеты кошелька), базовая монета.
    pub input: Decimal,
    pub output: Decimal,
    /// Курсы на момент сделки (из графа адаптера): USDT за 1 базовой и за 1 BNB.
    pub base_usdt: Decimal,
    pub bnb_usdt: Decimal,
}
