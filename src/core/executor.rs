//! Файл: core/executor.rs
//! Что делает: сделка целиком (шаг 5 ТЗ): предпроверка → покупка на A → перевод на B → продажа на B.
//! После каждого шага результат пересчитывается по факту. Не исполнилась покупка — сделка закрыта
//! без монет (стартовое правило шлюза: отменить, больше не пробовать — повтор только по правилу).
//! Покупка исполнилась частично — пересчёт прибыли на фактический объём (ТЗ, шаг 5): не проходит порог —
//! монеты на A уходят в режим застрявшей монеты (stuck.rs). Вывод отклонён — то же. Продажа: выгодно
//! сейчас — продаём; иначе — режим застрявшей монеты (решение 37). По одному направлению — одна сделка.
//! Кто вызывает: core/candidates.rs (прошедший кандидат).
//! От чего зависит: precheck.rs, trade_ops.rs, transfers.rs, fills.rs, ledger.rs, stuck.rs, sell.rs, deal.rs.
//! Настройки: через вызываемые модули.

use chrono::Utc;
use rust_decimal::Decimal;

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
    if r.filled_qty < plan.qty
        && let Err(why) = recheck_partial(core, &deal).await
    {
        deal.note(format!("Частичная покупка: {why}"));
        return super::stuck::handle(core, deal).await;
    }
    // Перевод на B.
    deal.step = Step::Transfer;
    super::ledger::save(core, &deal).await;
    match super::transfers::send(core, &deal.id, &d.buy_on, &d.sell_on, &d.asset, deal.qty).await {
        Ok((sent, credited)) => {
            super::fills::transfer(core, &mut deal, &sent, &credited, &d.sell_on, r.avg_price).await
        }
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

/// Частичная покупка: пересчёт полного расчёта на фактический объём с фактической ценой покупки.
/// Ошибка — текст: прибыль ниже порога или объём не проходит минимумы.
async fn recheck_partial(core: &Core, deal: &Deal) -> Result<(), String> {
    let cx = super::dirctx::DirCtx::build(core, &deal.direction)?;
    let c = super::costs::evaluate(&core.inner.deps, &cx, deal.qty).await?;
    // В расчёте покупка по текущей котировке — заменяем её фактом.
    let profit = c.profit_usd + c.invested - deal.invested - deal.spent;
    let s = &core.inner.deps.settings;
    let min = s
        .decimal("money.min_profit_usd", Default::default())
        .map_err(|e| e.message)?;
    let pct = s
        .decimal("money.min_profit_pct", Default::default())
        .map_err(|e| e.message)?;
    let profit_pct = if deal.invested.is_zero() {
        Decimal::ZERO
    } else {
        profit / deal.invested * Decimal::ONE_HUNDRED
    };
    if profit < min || profit_pct < pct {
        return Err(format!(
            "на фактический объём {} прибыль {} USDT ({}%) ниже порога",
            deal.qty,
            profit.round_dp(4),
            profit_pct.round_dp(4)
        ));
    }
    Ok(())
}
