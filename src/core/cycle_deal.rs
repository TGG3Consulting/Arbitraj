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
//!   Halted до ручной проверки, площадка на паузе.
//!
//! Кто вызывает: core/cycle_watch.rs.
//! От чего зависит: core/{cycle_plan, ledger, deal, executor (buy_gate), coin_key, rules_apply, gas_stats, risk}.rs,
//! gateway, core/types.
//! Настройки: через вызываемые (dry.enabled, signal.route_cooldown_s, правила шлюза).

use rust_decimal::Decimal;

use crate::core::types::CycleTradeRequest;
use crate::gateway::errors::{ErrorCode, GatewayError};

use super::cycle_plan::{self, CyclePlan};
use super::deal::Step;
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
    let res = match super::dex_account::venue_deal(core, &deal.id, &v).await {
        Ok(x) => x.dex_cycle_trade(req).await,
        Err(e) => Err(crate::gateway::errors::GatewayError::new(
            crate::gateway::errors::ErrorCode::NotConfigured,
            Some(v.clone()),
            e,
        )),
    };
    match res {
        Ok(r) => {
            if let Some(tx) = &r.trade.chain {
                super::gas_stats::record_tx(core, &d.asset, tx).await;
            }
            super::risk::on_ok(core, &v);
            deal.invested = r.input * r.base_usdt;
            deal.proceeds = r.output * r.base_usdt;
            // Решение 219: комиссия — список; у цикла DEX — газ в BNB.
            let gas = r.trade.fee_in(super::convert::GAS_TICKER);
            deal.spent += gas * r.bnb_usdt;
            let by = r.trade.executor.clone().unwrap_or_default();
            deal.note(format!(
                "Цикл исполнен: вход {} → выход {} {} (газ {} BNB); исполнил: {by}",
                r.input, r.output, d.asset.ticker, gas
            ));
            deal.buy = Some(r.trade);
            super::ledger::finish(core, &mut deal, Step::Closed, "цикл DEX ↔ DEX исполнен").await;
            Outcome::Done
        }
        Err(e) if e.tx.is_none() && e.code != ErrorCode::OutcomeUnknown => {
            super::ledger::discard(core, &deal.id);
            Outcome::Refused(e.message)
        }
        Err(e) => {
            sent_failed(core, &mut deal, &plan, e).await;
            Outcome::Done
        }
    }
}

/// Отказ после отправки: правило шлюза (без повтора), газ в затраты, сделка Closed или Halted (исход неизвестен).
async fn sent_failed(core: &Core, deal: &mut super::deal::Deal, plan: &CyclePlan, e: GatewayError) {
    let v = deal.direction.buy_on.clone();
    let _ = super::rules_apply::on_error(core, "Цикл DEX ↔ DEX", &v, Some(&deal.id), &e, 1).await;
    if let Some(tx) = &e.tx {
        super::gas_stats::record_tx(core, &deal.direction.asset, tx).await;
        let gas = tx.gas_bnb.unwrap_or(Decimal::ZERO) + tx.cancel_gas_bnb.unwrap_or(Decimal::ZERO);
        deal.spent += gas * plan.quote.bnb_usdt;
        deal.note(format!("Газ транзакции цикла: {gas} BNB"));
    }
    let (step, why) = if e.code == ErrorCode::OutcomeUnknown {
        (
            Step::Halted,
            format!("цикл: исход неизвестен — ручная проверка ({})", e.message),
        )
    } else {
        (Step::Closed, format!("цикл не исполнился: {}", e.message))
    };
    super::ledger::finish(core, deal, step, &why).await;
}
