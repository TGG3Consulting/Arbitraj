//! Файл: core/cycle_deal.rs
//! Что делает: сделка цикла DEX ↔ DEX (решение 124) — короткий исполнитель ядра вместо executor.rs: у цикла нет
//! перевода, депозита, зачисления, режима застрявшей монеты, частичной покупки и возврата USDT — деньги не покидают
//! кошелёк, цикл атомарен (весь в блоке или ничего). Шаги: замок «одна сделка на монету» по базовой монете (решение
//! 118) → третий замок сухого режима (как buy_gate) → сделка в учёте (ledger::open; направление: базовая, buy_on =
//! sell_on = площадка DEX) → ОДИН вызов Venue::dex_cycle_trade (повторов нет — стартовое правило Тиграна) →
//! - исполнено: учёт по факту (вложено = вход × курс базовой, выручка = выход × курс, затраты = газ × курс BNB — курсы
//!   адаптера по его пулам), ledger::finish Closed (план ↔ факт, итог дня, deal.closed);
//! - отказ ДО отправки (нет транзакции: цена ушла, проба откатилась, газ съел прибыль): сделки не было — запись
//!   убирается из учёта (ledger::discard), причина — в журнал (как попытка BingX ↔ DEX, закрытая до ордера, только
//!   журнал), вызывающий пишет её один раз на смену причины;
//! - отказ ПОСЛЕ отправки (откат в блоке, не попала в блок): правило шлюза (rules_apply.rs: журнал, exec.*, счётчик
//!   ошибок, StopVenue/StopAll; повтор не делаем), газ — в затраты, сделка Closed; исход неизвестен (OutcomeUnknown) —
//!   площадка на паузе, бот выясняет исход сам (cycle_outcome.rs, решение 233, дополнение, п. 4): «в блоке» — учёт по
//!   факту и Closed, как исполненный; «окончательно не исполнено» — Closed, денег не стоил; откат в блоке — газ в
//!   затраты, Closed; не выяснен (нет «чем найти», сроки не заданы, человек, срок вышел) — Halted до ручной проверки.
//!
//! Кто вызывает: core/cycle_watch.rs.
//! От чего зависит: core/{cycle_plan, cycle_outcome, ledger, deal, executor (buy_gate), coin_key, rules_apply, gas_stats, risk}.rs,
//! gateway, core/types.
//! Настройки: через вызываемые (dry.enabled, signal.route_cooldown_s, правила шлюза, risk.outcome_poll_s, risk.outcome_max_s).

use std::sync::Arc;

use rust_decimal::Decimal;

use crate::core::types::{CycleTradeRequest, CycleTradeResult};
use crate::gateway::contract::Venue;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::cycle_outcome;
use super::cycle_plan::{self, CyclePlan};
use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Что вышло.
pub enum Outcome {
    /// Сделка была (исполнена, откатилась, не попала в блок, исход неизвестен) — в учёте и на «Сделках».
    Done,
    /// Сделки не было — причина (для журнала).
    Refused(String),
    /// По базовой монете уже идёт сделка.
    Busy,
}

/// Ведёт сделку цикла. Принимает: ядро, план. Возвращает: Outcome. Ошибок нет.
pub async fn run(core: &Core, plan: CyclePlan) -> Outcome {
    let d = cycle_plan::direction(&plan.cycle);
    let key = super::coin_key::CoinKey::of(&d.asset);
    if !lk(&core.inner.active).insert(key.clone()) {
        return Outcome::Busy;
    }
    let out = deal(core, plan).await;
    lk(&core.inner.active).remove(&key);
    out
}

/// Тело run (под замком монеты).
async fn deal(core: &Core, plan: CyclePlan) -> Outcome {
    if let Err(why) = super::executor::buy_gate(core) {
        return Outcome::Refused(why);
    }
    let d = cycle_plan::direction(&plan.cycle);
    let v = d.buy_on.clone();
    // Решение 132: цикл — кошельком строки «DEX ↔ DEX»; нет строки — цикл не исполняется.
    let acc = match super::dex_account::of_direction(&core.inner.deps, &d).await {
        Ok(a) => a,
        Err(why) => return Outcome::Refused(why),
    };
    let plan_json = serde_json::to_value(&plan).unwrap_or_default();
    let mut deal = super::ledger::open(core, &d, plan_json, plan.profit_usd, acc);
    deal.note(format!(
        "План цикла DEX ↔ DEX: вход {} {t}, ожидаемо {} {t}, граница {} {t}, чистая {} {t} (≈ {} USDT), {}",
        plan.input,
        plan.quote.output,
        plan.min_out,
        plan.quote.net_base,
        plan.profit_usd.round_dp(4),
        plan.quote.form,
        t = d.asset.ticker,
    ));
    let req = CycleTradeRequest {
        venue: v.clone(),
        deal_id: deal.id.clone(),
        cycle: plan.cycle.clone(),
        input: plan.input,
        min_out: plan.min_out,
    };
    let x = match super::dex_account::venue_deal(core, &deal.id, &v).await {
        Ok(x) => x,
        Err(why) => {
            super::ledger::discard(core, &deal.id);
            return Outcome::Refused(why);
        }
    };
    match x.dex_cycle_trade(req.clone()).await {
        Ok(r) => {
            super::risk::on_ok(core, &v);
            filled(core, &mut deal, r).await;
            Outcome::Done
        }
        Err(e) if e.tx.is_none() && e.code != ErrorCode::OutcomeUnknown => {
            super::ledger::discard(core, &deal.id);
            Outcome::Refused(e.message)
        }
        Err(e) => {
            sent_failed(core, &mut deal, &plan, (&x, &req), e).await;
            Outcome::Done
        }
    }
}

/// Цикл исполнен: учёт по факту (вложено, выручка, газ — курсами адаптера), сделка Closed. Принимает: ядро, сделку,
/// итог. Ошибок нет.
async fn filled(core: &Core, deal: &mut Deal, r: CycleTradeResult) {
    if let Some(tx) = &r.trade.chain {
        super::gas_stats::record_tx(core, &deal.direction.asset, tx).await;
    }
    deal.invested = r.input * r.base_usdt;
    deal.proceeds = r.output * r.base_usdt;
    // Решение 219: комиссия — список; у цикла DEX — газ в BNB.
    let gas = r.trade.fee_in(super::convert::GAS_TICKER);
    deal.spent += gas * r.bnb_usdt;
    let by = r.trade.executor.clone().unwrap_or_default();
    deal.note(format!(
        "Цикл исполнен: вход {} → выход {} {} (газ {} BNB); исполнил: {by}",
        r.input, r.output, deal.direction.asset.ticker, gas
    ));
    deal.buy = Some(r.trade);
    super::ledger::finish(core, deal, Step::Closed, "цикл DEX ↔ DEX исполнен").await;
}

/// Отказ после отправки: правило шлюза (без повтора); исход известен (откат, не попала) — газ в затраты, Closed;
/// исход неизвестен — бот выясняет его сам (cycle_outcome.rs, решение 233, дополнение, п. 4), затем settle_unknown.
async fn sent_failed(
    core: &Core,
    deal: &mut Deal,
    plan: &CyclePlan,
    (x, req): (&Arc<dyn Venue>, &CycleTradeRequest),
    e: GatewayError,
) {
    let v = deal.direction.buy_on.clone();
    let _ = super::rules_apply::on_error(core, cycle_outcome::OP, &v, Some(&deal.id), &e, 1).await;
    if e.code != ErrorCode::OutcomeUnknown {
        spend(core, deal, plan.quote.bnb_usdt, &e).await;
        let why = format!("цикл не исполнился: {}", e.message);
        return super::ledger::finish(core, deal, Step::Closed, &why).await;
    }
    let found = cycle_outcome::resolve(core, &deal.id, &v, x, req, &e).await;
    settle_unknown(core, deal, plan.quote.bnb_usdt, &e, found).await;
}

/// Что делает: сделка цикла после выяснения исхода (решение 233, дополнение, п. 4). «В блоке» — как обычный исполненный
/// цикл (filled); «окончательно не исполнено» — цикл не исполнен, денег не стоил, Closed; откат в блоке — газ найденной
/// транзакции в затраты, Closed; nonce занят нашей отменой в блоке (TxNotIncluded с хешем отмены) — газ отмены в
/// затраты (spend: cancel_gas_bnb), Closed; не выяснен — газ из исходной ошибки, Halted (ручная проверка), как раньше. Газ исходной
/// ошибки при найденном итоге не считается: найденный итог несёт полный факт (иначе газ вошёл бы дважды).
/// Принимает: ядро, сделку, курс BNB плана, исходную ошибку, итог выяснения. Ошибок нет.
pub async fn settle_unknown(
    core: &Core,
    deal: &mut Deal,
    bnb_usdt: Decimal,
    e: &GatewayError,
    found: Option<GatewayResult<CycleTradeResult>>,
) {
    match found {
        Some(Ok(r)) if cycle_outcome::not_executed(&r) => {
            let why = r.trade.executor.clone().unwrap_or_default();
            deal.note(format!(
                "Исход цикла выяснен ботом: не исполнен, газа нет ({why})"
            ));
            deal.buy = Some(r.trade);
            let step_why = "цикл не исполнен — транзакции не в блоке, денег не стоил";
            super::ledger::finish(core, deal, Step::Closed, step_why).await;
        }
        Some(Ok(r)) => {
            super::risk::on_ok(core, &deal.direction.buy_on);
            deal.note(format!(
                "Исход цикла выяснен ботом: в блоке, tx {}",
                r.trade.venue_ref
            ));
            filled(core, deal, r).await;
        }
        Some(Err(x)) => {
            spend(core, deal, bnb_usdt, &x).await;
            let why = format!("цикл не исполнился (выяснено ботом): {}", x.message);
            super::ledger::finish(core, deal, Step::Closed, &why).await;
        }
        None => {
            spend(core, deal, bnb_usdt, e).await;
            let why = format!("цикл: исход неизвестен — ручная проверка ({})", e.message);
            super::ledger::finish(core, deal, Step::Halted, &why).await;
        }
    }
}

/// Газ транзакции ошибки (и отмены, занявшей nonce) — в затраты по курсу BNB плана и в статистику газа. Ошибок нет.
async fn spend(core: &Core, deal: &mut Deal, bnb_usdt: Decimal, e: &GatewayError) {
    if let Some(tx) = &e.tx {
        super::gas_stats::record_tx(core, &deal.direction.asset, tx).await;
        let gas = tx.gas_bnb.unwrap_or(Decimal::ZERO) + tx.cancel_gas_bnb.unwrap_or(Decimal::ZERO);
        deal.spent += gas * bnb_usdt;
        deal.note(format!("Газ транзакции цикла: {gas} BNB"));
    }
}
