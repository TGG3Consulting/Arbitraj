//! Файл: core/uncapped_replay.rs
//! Что делает: решение 218 — источник данных полного расчёта (costs_source.rs Source) для пересчёта старых записей
//! «без потолка» по исполненным сделкам (посчитаны после покупки, basis live_after_buy — занижены, решение 217). Модель
//! затрат и поиск — те же функции ядра (costs.rs evaluate_with, size_uncapped.rs); отличаются только данные:
//! - сторона биржи — стакан из записанного окна сделки на момент решения о входе (core/window_at.rs), котировка и
//!   худшая цена — тем же кодом, что у живой и замороженной котировки бирж (gateway/frozen.rs FrozenBook, walk, deepest);
//!   комиссия тейкера — ставка из плана сделки (комиссия котировки плана / её база: от количества, если комиссия в
//!   монете, иначе от суммы);
//! - сторона DEX (только биржа → DEX) — ИЗ ПЛАНА СДЕЛКИ: средняя цена котировки продажи плана как постоянная цена без
//!   кривой (глубина пулов на момент решения не записана) — приблизительно; затраты котировки плана: газ (газ обмена,
//!   approve, обёртки, чаевые канала) — как в плане, прочие — пропорционально количеству;
//! - вывод с A — затраты стадии «перевод» плана: комиссия вывода — как в плане (постоянная), налог перевода — той же
//!   долей от отправленного;
//! - возврат USDT — затраты стадии «возврат USDT» плана (как в плане);
//! - курсы валют затрат (BNB и т. п.) — из строк плана (USDT строки / количество).
//!
//! Правил пары биржи (шаг, minQty, minNotional…) и минимумов вывода и депозита на момент сделки в базе нет — они не
//! применяются (пары нет в контексте), о чём пишется в записи; шаг поиска — uncapped_recompute_cfg.rs step_of.
//! Кто вызывает: core/uncapped_recompute_ctx.rs, тесты (tests/uncapped_recompute.rs).
//! От чего зависит: costs_source.rs, uncapped_replay_plan.rs (план сделки, ставка комиссии, котировка DEX из плана),
//! convert.rs (Rates, is_gas), gateway/frozen.rs, core/types.
//! Настройки: нет (настройки модели читает uncapped_recompute_ctx.rs).

use std::collections::HashMap;

use rust_decimal::Decimal;

use crate::core::types::{Cost, CostKind, Direction, OrderBook, Quote, Side};
use crate::gateway::frozen::{FrozenBook, deepest};

use super::convert::{Rates, is_gas};
use super::costs_source::Source;
use super::dirctx::DirCtx;
use super::size::Fail;
pub use super::uncapped_replay_plan::{Plan, PlanLine};
use super::uncapped_replay_plan::{as_cost, dex_quote, fee_of};

/// Сторона продажи.
pub enum Sell {
    /// Биржа: стакан окна.
    Book(FrozenBook),
    /// DEX: котировка продажи плана как постоянная цена (приблизительно).
    DexPlan(Quote),
}

/// Источник пересчёта (см. шапку).
pub struct Replay {
    pub buy: FrozenBook,
    pub sell: Sell,
    /// Затраты стадии «перевод» и «возврат USDT» плана, отправлено с A по плану.
    pub transfer: Vec<PlanLine>,
    pub sent_plan: Decimal,
    pub returns: Vec<PlanLine>,
    /// Курс валюты затрат из плана, USDT за единицу.
    pub prices: HashMap<String, Decimal>,
}

impl Replay {
    /// Что делает: источник по плану и стаканам окна. Принимает: направление, план, стакан A (покупка), сторону B
    /// (стакан окна или None — DEX, тогда котировка плана), котируемые монеты A и B (venue.quote_asset).
    /// Возвращает: Replay. Ошибки: текст — комиссия плана без базы.
    pub fn new(
        d: &Direction,
        plan: &Plan,
        a: OrderBook,
        b: Option<OrderBook>,
        quote: [&str; 2],
    ) -> Result<Replay, String> {
        let coin = &d.asset.ticker;
        let book = |ob: OrderBook, side, q: &Quote, qa: &str| -> Result<FrozenBook, String> {
            Ok(FrozenBook {
                venue: ob.venue.clone(),
                asset: d.asset.clone(),
                side,
                fee: fee_of(q, coin, qa)?,
                taken_at: ob.received_at,
                book: ob,
            })
        };
        let buy = book(a, Side::Buy, &plan.buy_quote, quote[0])?;
        let sell = match b {
            Some(ob) => Sell::Book(book(ob, Side::Sell, &plan.sell_quote, quote[1])?),
            None => Sell::DexPlan(plan.sell_quote.clone()),
        };
        let stage = |s: &str| -> Vec<PlanLine> {
            plan.costs
                .iter()
                .filter(|l| l.stage == s)
                .cloned()
                .collect()
        };
        let transfer = stage("перевод");
        // Отправлено с A по плану = приход + затраты перевода в монете (они уменьшили количество).
        let in_coin: Decimal = transfer
            .iter()
            .filter(|l| &l.asset == coin && !is_gas(l.kind))
            .map(|l| l.amount)
            .sum();
        let prices = plan
            .costs
            .iter()
            .filter_map(|l| match l.usdt {
                Some(u) if !l.amount.is_zero() => Some((l.asset.clone(), u / l.amount)),
                _ => None,
            })
            .collect();
        Ok(Replay {
            buy,
            sell,
            sent_plan: plan.arrive + in_coin,
            transfer,
            returns: stage("возврат USDT"),
            prices,
        })
    }
}

impl Source for Replay {
    /// Покупка — стакан окна A; продажа — стакан окна B или котировка плана (DEX). Ошибок нет.
    async fn quote(&self, _d: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        Ok(match (sell, &self.sell) {
            (false, _) => self.buy.price(qty),
            (true, Sell::Book(b)) => b.price(qty),
            (true, Sell::DexPlan(p)) => dex_quote(p, qty),
        })
    }

    /// Биржа — самый глубокий нужный уровень стакана окна; DEX — средняя цена (правил пары у DEX нет).
    /// Ошибки: Stop — глубины стакана окна не хватает.
    fn worst(
        &self,
        d: &Direction,
        sell: bool,
        qty: Decimal,
        avg: Decimal,
    ) -> Result<Decimal, Fail> {
        let book = match (sell, &self.sell) {
            (false, _) => &self.buy.book,
            (true, Sell::Book(b)) => &b.book,
            (true, Sell::DexPlan(_)) => return Ok(avg),
        };
        let (v, side) = if sell {
            (&d.sell_on, Side::Sell)
        } else {
            (&d.buy_on, Side::Buy)
        };
        deepest(book, side, qty)
            .ok_or_else(|| Fail::Stop(format!("{}: глубины стакана окна не хватает на {qty}", v.0)))
    }

    /// Затраты «перевод» плана: налог перевода — той же долей от ушедшего в сеть (вывод минус комиссия вывода), прочее —
    /// как в плане. Ошибок нет.
    async fn withdraw(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        let coin = &cx.dir.asset.ticker;
        let fee: Decimal = self
            .transfer
            .iter()
            .filter(|l| l.kind == CostKind::WithdrawFee && &l.asset == coin)
            .map(|l| l.amount)
            .sum();
        let plan_net = self.sent_plan - fee;
        Ok(self
            .transfer
            .iter()
            .map(|l| match l.kind {
                CostKind::TransferTax if !plan_net.is_zero() => {
                    as_cost(l, (amount - fee).max(Decimal::ZERO) * l.amount / plan_net)
                }
                _ => as_cost(l, l.amount),
            })
            .collect())
    }

    /// Затраты «возврат USDT» плана — как в плане. Ошибок нет.
    async fn ret(&self, _cx: &DirCtx, _amount: Decimal) -> Result<Vec<Cost>, Fail> {
        Ok(self.returns.iter().map(|l| as_cost(l, l.amount)).collect())
    }

    /// Курсы из плана. Ошибки: текст — валюты затраты в плане нет.
    async fn rates(
        &self,
        cx: &DirCtx,
        coin: &str,
        coin_price: Decimal,
        must: &[Cost],
    ) -> Result<Rates, String> {
        let mut prices = HashMap::new();
        for c in must
            .iter()
            .filter(|c| !cx.quote.contains(&c.asset) && c.asset != coin && !c.amount.is_zero())
        {
            let p = self.prices.get(&c.asset).ok_or_else(|| {
                format!(
                    "затрата {:?} в {} — курса в плане сделки нет",
                    c.kind, c.asset
                )
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
