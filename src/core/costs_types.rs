//! Файл: core/costs_types.rs
//! Что делает: типы полного расчёта направления (core/costs.rs): строка затрат и расчёт на объём — с тем, как
//! подобран объём (решение 66). Вынесены из costs.rs, чтобы файл расчёта оставался коротким. Calc::pick_note — выбор
//! исполнителя DEX из котировок (решение 123) для хронологии сделки (executor.rs). Решение 219: комиссия покупки в
//! котируемой монете по худшей цене (spend_fee) — часть списания spend_max; предел цены ордера под неё (order_cap).
//! Решение 262: худшая цена покупки плана (buy_worst) — лимит ордера покупки на бирже.
//! Кто вызывает: core/costs.rs (собирает), profit.rs, candidates.rs, executor.rs, partial.rs, интерфейс (JSON).
//! От чего зависит: core/types, core/size.rs (SizeChoice).
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{CostKind, Direction, Quote, RouteStep};

use super::size::SizeChoice;

/// Этап строк «возврат USDT» расчёта (costs.rs; читают plan_costs.rs, precheck.rs, uncapped_replay_plan.rs, отчёт
/// «Итоги сделок» report/rows_text.rs — решение 257: одна строка на всех).
pub const RETURN_STAGE: &str = "возврат USDT";

/// Одна строка затрат расчёта.
#[derive(Debug, Clone, Serialize)]
pub struct CostLine {
    pub kind: CostKind,
    /// Где возникла: «A» (покупка), «перевод», «B» (продажа), «возврат USDT», «ядро» (запас r).
    pub stage: &'static str,
    pub amount: Decimal,
    pub asset: String,
    /// В USDT по живым ценам. None — только у затраты «в цене» без живой цены её валюты (показ;
    /// в итог она не входит — уже в средней цене котировки).
    pub usdt: Option<Decimal>,
    /// Уже в средней цене котировки — повторно не вычитается.
    pub in_price: bool,
    /// Затрата в монете: уменьшает количество (до продажи доходит меньше) — повторно не вычитается.
    pub in_qty: bool,
    /// Шаг маршрута обмена (решение 52): «USDT → BNB», «BNB → монета»; None — не шаг обмена.
    pub step: Option<RouteStep>,
}

/// Расчёт направления на объём.
#[derive(Debug, Clone, Serialize)]
pub struct Calc {
    pub direction: Direction,
    /// Сколько покупаем на A.
    pub qty: Decimal,
    /// Сколько дойдёт до B.
    pub arrive: Decimal,
    /// Сколько продаём на B: приход вниз до шага пары биржи B (у DEX — весь приход).
    pub sell_qty: Decimal,
    pub buy_avg: Decimal,
    pub sell_avg: Decimal,
    /// Вложено: qty × buy_avg.
    pub invested: Decimal,
    /// Выручка: sell_qty × sell_avg (остаток меньше шага B в выручку не входит).
    pub proceeds: Decimal,
    pub costs: Vec<CostLine>,
    /// Затраты, вычитаемые из прибыли (не в цене и не в количестве), USDT.
    pub costs_usdt: Decimal,
    pub profit_usd: Decimal,
    pub profit_pct: Decimal,
    pub transit_s: Decimal,
    pub sigma: Decimal,
    /// Котировки — для худшей цены ордеров и маршрута DEX при исполнении.
    pub buy_quote: Quote,
    pub sell_quote: Quote,
    pub at: DateTime<Utc>,
    /// Наибольшее списание по бюджету (решение 66): qty × худшая цена покупки — лимитная цена биржи (столько
    /// замораживает ордер) или средняя DEX с допуском exec.dex_slippage_pct (больше обмен не возьмёт) — плюс
    /// spend_fee (решение 219). Не больше бюджета.
    pub spend_max: Decimal,
    /// Решение 262: худшая цена покупки плана (worst_a расчёта, costs.rs) — у биржи самый глубокий уровень стакана
    /// расчёта, нужный на qty (у DEX — средняя с допуском). По ней посчитан spend_max. Лимит ордера покупки на бирже —
    /// ровно она (executor.rs → trade_ops.rs, ТЗ «Покупка на A»: «с ценой не выше худшего уровня из плана»).
    pub buy_worst: Decimal,
    /// Решение 219: торговая комиссия покупки, которую биржа берёт (и блокирует под ордер) в котируемой монете A, по
    /// худшей цене. Живой бот 08.11, MEXC 30004 «Insufficient position»: 76 из 78 отказов — объём подогнан ровно под
    /// свободный остаток (решение 103) без комиссии в USDT. Комиссия в монете или монетой скидки (paid_in) — 0.
    pub spend_fee: Decimal,
    /// Как подобран объём (решение 66): пределы, прибыль на бюджете, почему этот лучший. None — расчёт на заданный
    /// объём без подбора (partial.rs).
    pub size: Option<SizeChoice>,
}

/// Что делает: комиссия покупки в котируемой монете по худшей цене (решение 219): затраты котировки покупки не «в
/// цене», в котируемой монете `quote` и не монетой скидки (paid_in), пересчитанные с средней цены на худшую
/// (комиссия биржи — доля суммы сделки: по худшей цене ордер может списать столько).
/// Принимает: котировку покупки, тикер котируемой монеты A, худшую цену. Возвращает: сумму ≥ 0. Ошибок нет.
pub fn quote_fee_at_worst(q: &Quote, quote: &str, worst: Decimal) -> Decimal {
    let fee: Decimal = q
        .costs
        .iter()
        .filter(|c| !c.in_price && c.paid_in.is_none() && c.asset == quote)
        .map(|c| c.amount)
        .sum();
    if fee.is_zero() || q.avg_price.is_zero() || worst <= q.avg_price {
        return fee;
    }
    fee * worst / q.avg_price
}

/// Что делает: то же, что Calc::order_cap, по числам. Принимает: предел, списание плана, комиссию в котируемой монете.
/// Возвращает: cap × (spend_max − spend_fee) / spend_max; без комиссии или без списания — cap. Ошибок нет.
pub fn order_cap(cap: Decimal, spend_max: Decimal, spend_fee: Decimal) -> Decimal {
    if spend_fee.is_zero() || spend_max.is_zero() {
        return cap;
    }
    cap * (spend_max - spend_fee) / spend_max
}

impl Calc {
    /// Что делает: предел суммы ордера «количество × худшая цена» для проверки исполнителя (trade_ops::trade) при
    /// пределе списания `cap` (решение 219): из предела вычитается доля комиссии в котируемой монете — так ордер с
    /// комиссией не выходит за cap и при пересчитанной перед ордером худшей цене (комиссия растёт с ценой).
    /// Принимает: предел списания. Возвращает: cap × (spend_max − spend_fee) / spend_max; без комиссии — cap.
    /// Ошибок нет.
    pub fn order_cap(&self, cap: Decimal) -> Decimal {
        order_cap(cap, self.spend_max, self.spend_fee)
    }

    /// Выбор исполнителя DEX (решение 123, «лучший из двух»): тексты адаптера из обеих котировок — для хронологии
    /// сделки («; DEX покупка: KyberSwap … → свой»). Ядро текст не разбирает. Возвращает: строку (пусто — выбора не
    /// было). Ошибок нет.
    pub fn pick_note(&self) -> String {
        [("покупка", &self.buy_quote), ("продажа", &self.sell_quote)]
            .iter()
            .filter_map(|(side, q)| q.pick.as_ref().map(|p| format!("; DEX {side}: {p}")))
            .collect()
    }
}
