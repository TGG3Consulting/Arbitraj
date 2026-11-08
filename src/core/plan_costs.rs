//! Файл: core/plan_costs.rs
//! Что делает: затраты из плана сделки на любой объём — общая часть двух источников полного расчёта «без потолка»:
//! - пересчёт старых записей по окну (решение 218, core/uncapped_replay.rs): вывод с A, возврат USDT, курсы затрат;
//! - расчёт по сделке на данных момента решения (решение 237, core/frozen.rs Decided, источник Live со снимком):
//!   курсы затрат, а у DEX-стороны — вывод с A и возврат USDT (у биржи-стороны они — по контексту направления,
//!   снятому в момент решения: справочник сетей ядра).
//!
//! Правила (как в решении 218): вывод — затраты стадии «перевод» плана: комиссия вывода и газ перевода — как в плане
//! (постоянные), налог перевода — той же долей от ушедшего в сеть; возврат USDT — затраты стадии «возврат USDT» плана
//! (как в плане); курс валюты затрат — USDT строки плана / её количество (валюты нет в плане — расчёт на этом объёме
//! невозможен, причина в тексте; живой курс не подставляется). Решение 239: такая точка — «объём недопустим»
//! (Fail::TooLarge с меткой NOT_IN_PLAN: валюта появилась на большом объёме, например промежуточная монета маршрута
//! DEX), а не остановка всего расчёта; причина — в записи (core/plan_gap.rs).
//! Кто вызывает: core/uncapped_replay.rs, core/frozen.rs, core/costs_source.rs.
//! От чего зависит: uncapped_replay_plan.rs (PlanLine, as_cost), convert.rs (Rates, is_gas), costs_types.rs (CostLine).
//! Настройки: нет.

use std::collections::HashMap;

use rust_decimal::Decimal;

use crate::core::types::{Cost, CostKind};

use super::convert::{Rates, is_gas};
use super::costs_types::CostLine;
use super::dirctx::DirCtx;
use super::size::Fail;
use super::uncapped_replay_plan::{PlanLine, as_cost};

/// Решение 239: метка причины «валюты затраты в плане сделки нет» — по ней core/plan_gap.rs узнаёт такие точки.
pub const NOT_IN_PLAN: &str = "курса в плане сделки нет";

/// Затраты плана сделки (см. шапку).
#[derive(Debug, Clone)]
pub struct PlanCosts {
    /// Затраты стадии «перевод» плана и сколько отправлено с A по плану.
    pub transfer: Vec<PlanLine>,
    pub sent_plan: Decimal,
    /// Затраты стадии «возврат USDT» плана.
    pub returns: Vec<PlanLine>,
    /// Курс валюты затрат из плана, USDT за единицу.
    pub prices: HashMap<String, Decimal>,
}

impl PlanCosts {
    /// Что делает: из строк затрат плана. Принимает: монету сделки (тикер ядра), строки плана, приход на B по плану.
    /// Возвращает: PlanCosts. Ошибок нет.
    pub fn new(coin: &str, lines: &[PlanLine], arrive: Decimal) -> PlanCosts {
        let stage =
            |s: &str| -> Vec<PlanLine> { lines.iter().filter(|l| l.stage == s).cloned().collect() };
        let transfer = stage("перевод");
        // Отправлено с A по плану = приход + затраты перевода в монете (они уменьшили количество).
        let in_coin: Decimal = transfer
            .iter()
            .filter(|l| l.asset == coin && !is_gas(l.kind))
            .map(|l| l.amount)
            .sum();
        let prices = lines
            .iter()
            .filter_map(|l| match l.usdt {
                Some(u) if !l.amount.is_zero() => Some((l.asset.clone(), u / l.amount)),
                _ => None,
            })
            .collect();
        PlanCosts {
            sent_plan: arrive + in_coin,
            transfer,
            returns: stage("возврат USDT"),
            prices,
        }
    }

    /// Что делает: строки плана из расчёта ядра (core/costs_types.rs CostLine — Calc плана в памяти, решение 237).
    /// Принимает: строки затрат Calc. Возвращает: строки плана. Ошибок нет.
    pub fn lines_of(costs: &[CostLine]) -> Vec<PlanLine> {
        costs
            .iter()
            .map(|c| PlanLine {
                kind: c.kind,
                stage: c.stage.to_string(),
                amount: c.amount,
                asset: c.asset.clone(),
                usdt: c.usdt,
            })
            .collect()
    }

    /// Что делает: стоимость вывода с A на объём. Налог перевода — той же долей от ушедшего в сеть (вывод минус
    /// комиссия вывода), прочее — как в плане. Принимает: контекст, сумму вывода. Возвращает: затраты. Ошибок нет.
    pub fn withdraw(&self, cx: &DirCtx, amount: Decimal) -> Vec<Cost> {
        let coin = &cx.dir.asset.ticker;
        let fee: Decimal = self
            .transfer
            .iter()
            .filter(|l| l.kind == CostKind::WithdrawFee && &l.asset == coin)
            .map(|l| l.amount)
            .sum();
        let plan_net = self.sent_plan - fee;
        self.transfer
            .iter()
            .map(|l| match l.kind {
                CostKind::TransferTax if !plan_net.is_zero() => {
                    as_cost(l, (amount - fee).max(Decimal::ZERO) * l.amount / plan_net)
                }
                _ => as_cost(l, l.amount),
            })
            .collect()
    }

    /// Что делает: стоимость возврата USDT — как в плане. Возвращает: затраты. Ошибок нет.
    pub fn ret(&self) -> Vec<Cost> {
        self.returns.iter().map(|l| as_cost(l, l.amount)).collect()
    }

    /// Что делает: курсы валют затрат из плана. Принимает: контекст, тикер монеты, её цену, вычитаемые затраты.
    /// Возвращает: Rates. Ошибки: TooLarge (решение 239: объём недопустим) — валюты затраты в плане нет.
    pub fn rates(
        &self,
        cx: &DirCtx,
        coin: &str,
        coin_price: Decimal,
        must: &[Cost],
    ) -> Result<Rates, Fail> {
        let mut prices = HashMap::new();
        for c in must
            .iter()
            .filter(|c| !cx.quote.contains(&c.asset) && c.asset != coin && !c.amount.is_zero())
        {
            let p = self.prices.get(&c.asset).ok_or_else(|| {
                Fail::TooLarge(format!(
                    "затрата {:?} в {} — {NOT_IN_PLAN} (объём недопустим)",
                    c.kind, c.asset
                ))
            })?;
            prices.insert(c.asset.clone(), *p);
        }
        Ok(Rates {
            quote: cx.quote.clone(),
            coin: coin.to_string(),
            coin_price,
            prices,
        })
    }
}
