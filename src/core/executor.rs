//! Файл: core/executor.rs
//! Что делает: сделка целиком (шаг 5 ТЗ): предпроверка → покупка на A → перевод на B → продажа на B.
//! После каждого шага результат пересчитывается по факту. Не исполнилась покупка — сделка закрыта
//! без монет (стартовое правило шлюза: отменить, больше не пробовать — повтор только по правилу).
//! Покупка исполнилась частично — решение 48 (partial.rs): больший итог из «довезти и продать» и
//! «продать обратно на A». Вывод не состоялся — монеты на A, режим застрявшей монеты. Продажа: выгодно
//! (итог не ниже нуля, решение 47) — продаём; иначе — режим застрявшей монеты (решение 37). По одному направлению — одна сделка.
//! Исход покупки или перевода неизвестен (OutcomeUnknown) — сделка остановлена до ручной проверки (Step::Halted), без
//! застрявшей монеты, продажи и повторного перевода.
//! Кто вызывает: core/candidates.rs (прошедший кандидат).
//! От чего зависит: precheck.rs, trade_ops.rs, transfers.rs, fills.rs, ledger.rs, stuck.rs, sell.rs, deal.rs.
//! Настройки: через вызываемые модули.

use chrono::Utc;

use crate::core::types::{Direction, Side};
use crate::storage::journal::Level;

use super::costs::Calc;
use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Идентификатор сделки: время до мс + тикер, только буквы и цифры (так его примут ID ордера и вывода).
fn deal_id(d: &Direction) -> String {
    let t: String = d
        .asset
        .ticker
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    format!("{}{t}", Utc::now().format("%Y%m%d%H%M%S%3f"))
}

/// Кандидат прошёл расчёт — запускает сделку, если по направлению сейчас нет другой. Ошибок нет.
pub async fn accepted(core: &Core, calc: Calc) {
    let d = calc.direction.clone();
    if !lk(&core.inner.active).insert(d.clone()) {
        return;
    }
    let core = core.clone();
    tokio::spawn(async move {
        run(&core, d.clone()).await;
        lk(&core.inner.active).remove(&d);
    });
}

/// Ведёт сделку до конца или до передачи в режим застрявшей монеты.
async fn run(core: &Core, d: Direction) {
    let deps = &core.inner.deps;
    let plan = match super::precheck::run(core, &d).await {
        Ok(c) => c,
        Err(e) => {
            deps.log(
                Level::Info,
                "core.precheck",
                Some(&d.buy_on.0),
                format!(
                    "{} {} → {}: сделки нет — {e}",
                    d.asset.ticker, d.buy_on.0, d.sell_on.0
                ),
                None,
            )
            .await;
            return;
        }
    };
    let mut deal = Deal::new(
        deal_id(&d),
        d.clone(),
        serde_json::to_value(&plan).unwrap_or_default(),
        plan.profit_usd,
    );
    deal.note(format!(
        "План: объём {}, прибыль {} USDT",
        plan.qty,
        plan.profit_usd.round_dp(4)
    ));
    super::ledger::save(core, &deal).await;
    // Покупка на A.
    let bought = super::trade_ops::trade(
        core,
        &deal.id,
        &d.buy_on,
        &d.asset,
        Side::Buy,
        plan.qty,
        plan.buy_quote.route.clone(),
        plan.buy_quote.avg_price,
    )
    .await;
    let r = match bought {
        Ok(r) => r,
        Err(e) if e.outcome_unknown() => return unknown(core, &mut deal, "покупка", &e).await,
        Err(e) => {
            return super::ledger::finish(
                core,
                &mut deal,
                Step::Closed,
                &format!("покупка не исполнилась: {e}"),
            )
            .await;
        }
    };
    super::fills::trade(core, &mut deal, &r).await;
    deal.buy = Some(r.clone());
    let text = format!(
        "{}: куплено {} {} на {} по {}",
        deal.id, r.filled_qty, d.asset.ticker, d.buy_on.0, r.avg_price
    );
    deps.notify("deal.buy_filled", Some(&d.buy_on.0), &text)
        .await;
    if r.filled_qty < plan.qty {
        // Решение 48: больший итог из «довезти» и «продать на A»; застрявшая монета не включается.
        match super::partial::choose(core, &deal).await {
            super::partial::Choice::Deliver(x) => {
                deal.note(format!(
                    "Частичная покупка: довезти и продать (итог {} USDT)",
                    x.round_dp(4)
                ));
            }
            super::partial::Choice::SellBack(x) => {
                let why = format!(
                    "частичная покупка: продать на A выгоднее (итог {} USDT)",
                    x.round_dp(4)
                );
                deal.note(why.clone());
                return super::sell::market(core, deal, &why).await;
            }
            super::partial::Choice::Neither(e) => {
                let why = format!("частичная покупка: ни один путь не считается — {e}");
                return super::ledger::finish(core, &mut deal, Step::Halted, &why).await;
            }
        }
    }
    // Перевод на B.
    deal.step = Step::Transfer;
    super::ledger::save(core, &deal).await;
    match super::transfers::send(core, &deal.id, &d.buy_on, &d.sell_on, &d.asset, deal.qty).await {
        Ok((sent, credited)) => {
            super::fills::transfer(core, &mut deal, &sent, &credited, &d.sell_on, r.avg_price).await
        }
        Err(e) if e.outcome_unknown() => return unknown(core, &mut deal, "перевод", &e).await,
        Err(e) => {
            deal.step = Step::Stuck;
            deal.note(format!("Перевод не состоялся: {e}"));
            return super::stuck::handle(core, deal).await;
        }
    }
    deal.step = Step::Selling;
    super::ledger::save(core, &deal).await;
    super::sell::sell_or_stuck(core, deal).await;
}

/// Исход шага неизвестен (транзакция могла исполниться): сделка останавливается до ручной проверки — без режима
/// застрявшей монеты, без продажи и без второго перевода (повтор — риск двойного вывода). Уведомление deal.closed с
/// причиной (ledger.rs) и send.outcome_unknown (rules_apply.rs). Ошибок нет.
async fn unknown(core: &Core, deal: &mut Deal, op: &str, e: &super::op_error::OpError) {
    let why = format!("{op}: исход неизвестен — ручная проверка ({e})");
    super::ledger::finish(core, deal, Step::Halted, &why).await;
}
