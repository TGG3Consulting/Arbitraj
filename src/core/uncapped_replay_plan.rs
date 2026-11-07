//! Файл: core/uncapped_replay_plan.rs
//! Что делает: решение 218 — план сделки (core/costs_types.rs Calc в JSON, Deal.plan) для пересчёта старых записей
//! «без потолка» по окну (uncapped_replay.rs): строки затрат и котировки плана; ставка комиссии тейкера биржи по
//! котировке плана (fee_of); котировка DEX из плана на любой объём — постоянная цена плана, газ как в плане, прочие
//! затраты пропорционально (dex_quote) — приблизительно, кривой пулов на момент решения нет.
//! Кто вызывает: core/uncapped_replay.rs, core/uncapped_recompute_ctx.rs, uncapped_recompute_cfg.rs (Plan).
//! От чего зависит: convert.rs (is_gas), gateway/frozen.rs (BookFee), core/types, serde.
//! Настройки: нет.

use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::Value;

use crate::core::types::{Cost, CostKind, Quote};
use crate::gateway::frozen::BookFee;

use super::convert::is_gas;

/// Строка затрат плана (core/costs_types.rs CostLine в JSON).
#[derive(Debug, Clone, Deserialize)]
pub struct PlanLine {
    pub kind: CostKind,
    pub stage: String,
    pub amount: Decimal,
    pub asset: String,
    pub usdt: Option<Decimal>,
}

/// Что нужно пересчёту из плана сделки (core/costs_types.rs Calc в JSON).
#[derive(Debug, Clone, Deserialize)]
pub struct Plan {
    pub qty: Decimal,
    pub arrive: Decimal,
    pub sell_qty: Decimal,
    pub sigma: Decimal,
    pub transit_s: Decimal,
    pub costs: Vec<PlanLine>,
    pub buy_quote: Quote,
    pub sell_quote: Quote,
}

impl Plan {
    /// Разбор плана сделки. Ошибки: текст — план не того вида (поле не найдено или не разобрано).
    pub fn parse(v: &Value) -> Result<Plan, String> {
        Plan::deserialize(v).map_err(|e| format!("план сделки не читается: {e}"))
    }

    /// Возврат USDT в каждой сделке был в плане (строки стадии «возврат USDT»). Ошибок нет.
    pub fn returns(&self) -> bool {
        self.costs.iter().any(|l| l.stage == "возврат USDT")
    }
}

/// План комиссии тейкера биржи по котировке плана: торговая комиссия в монете — от количества, иначе — от суммы.
/// Комиссии в котировке нет — ставка 0 (в котируемой монете стороны `quote`). Ошибки: текст — база комиссии нулевая.
pub fn fee_of(q: &Quote, coin: &str, quote: &str) -> Result<BookFee, String> {
    let fee = q
        .costs
        .iter()
        .find(|c| c.kind == CostKind::TradeFee && !c.in_price);
    let Some(f) = fee else {
        return Ok(BookFee {
            rate: Decimal::ZERO,
            per_coin: false,
            asset: quote.to_string(),
        });
    };
    let per_coin = f.asset == coin;
    let base = if per_coin {
        q.fillable_qty
    } else {
        q.fillable_qty * q.avg_price
    };
    if base.is_zero() {
        return Err(format!(
            "{}: комиссия плана без объёма — ставку не вычислить",
            q.venue.0
        ));
    }
    Ok(BookFee {
        rate: f.amount / base,
        per_coin,
        asset: f.asset.clone(),
    })
}

/// Котировка DEX из плана на объём: постоянная цена плана, газ — как в плане, прочие затраты — пропорционально.
/// Ошибок нет.
pub fn dex_quote(p: &Quote, qty: Decimal) -> Quote {
    let k = if p.qty.is_zero() {
        Decimal::ZERO
    } else {
        qty / p.qty
    };
    let costs = p
        .costs
        .iter()
        .map(|c| {
            let mut c = c.clone();
            if !is_gas(c.kind) {
                c.amount *= k;
            }
            c
        })
        .collect();
    Quote {
        qty,
        fillable_qty: qty,
        costs,
        ..p.clone()
    }
}

/// Затраты плана как затраты расчёта. Ошибок нет.
pub fn as_cost(l: &PlanLine, amount: Decimal) -> Cost {
    Cost::new(l.kind, amount, l.asset.clone(), false)
}
