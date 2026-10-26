//! Файл: core/executor.rs
//! Что делает: сделка целиком (шаг 5 ТЗ): предпроверка → покупка на A → перевод на B → продажа на B.
//! После каждого шага результат пересчитывается по факту. Не исполнилась покупка — сделка закрыта
//! без монет (стартовое правило шлюза: отменить, больше не пробовать — повтор только по правилу).
//! Покупка исполнилась частично — решение 48 (partial.rs): больший итог из «довезти и продать» и
//! «продать обратно на A». Вывод не состоялся — монеты на A, режим застрявшей монеты. Продажа: выгодно
//! (итог не ниже нуля, решение 47) — продаём; иначе — режим застрявшей монеты (решение 37). По одному направлению — одна сделка.
//! Исход покупки или перевода неизвестен (OutcomeUnknown) — сделка остановлена до ручной проверки (Step::Halted), без
//! застрявшей монеты, продажи и повторного перевода.
//! Сухой режим (решение 74, dry.rs) — второй замок на входе: сделка не начинается, даже если кандидата передали сюда
//! (первый замок — candidates.rs); предпроверка считает план в режиме Live (виртуального бюджета у сделки нет); перед
//! покупкой — третий замок (buy_gate). После покупки сделка доводится как обычно.
//! Кто вызывает: core/candidates.rs (прошедший кандидат).
//! От чего зависит: precheck.rs, trade_ops.rs, transfers.rs, fills.rs, ledger.rs, stuck.rs, sell.rs, deal.rs.
//! Настройки: через вызываемые модули.

use chrono::Utc;

use crate::core::types::{Direction, Side};
use crate::gateway::urgency::{self, Urgency};
use crate::storage::journal::Level;

use super::costs::Calc;
use super::deal::{Deal, Step};
use super::engine::{Core, lk, rd};

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

/// Что вышло на входе исполнителя.
#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    /// Сделка запущена.
    Started,
    /// Сухой режим (решение 74): ничего не делается — причина.
    Dry(String),
    /// Направление больше не торгуемо (чёрный список, налоги не проверены, монеты нет в справочнике).
    NotTradable,
    /// По направлению уже идёт сделка.
    Busy,
}

/// Кандидат прошёл расчёт — запускает сделку, если режим не сухой (решение 74), направление по-прежнему торгуемо
/// (есть в directions() монеты: за время расчёта монета могла уйти в чёрный список или в «налоги не проверены») и
/// по нему сейчас нет другой. Вся сделка — класс срочности Trade (gateway/urgency.rs, решение 61).
/// Возвращает: Start — что вышло. Ошибок нет.
pub async fn accepted(core: &Core, calc: Calc) -> Start {
    // Сухой режим проверяется первым: до справочника, учёта и шлюза — ни одного действия.
    if let Some(why) = super::dry::blocks(&core.inner.deps.settings) {
        return Start::Dry(why);
    }
    let d = calc.direction.clone();
    let tradable = rd(&core.inner.coins)
        .iter()
        .find(|c| c.asset == d.asset)
        .is_some_and(|c| c.directions().contains(&d));
    if !tradable {
        return Start::NotTradable;
    }
    if !lk(&core.inner.active).insert(d.clone()) {
        return Start::Busy;
    }
    let core = core.clone();
    tokio::spawn(urgency::scope(Urgency::Trade, async move {
        run(&core, d.clone()).await;
        lk(&core.inner.active).remove(&d);
    }));
    Start::Started
}

/// Замок перед покупкой (решение 74): сухой режим включён — покупки нет. Ошибки: текст причины.
pub fn buy_gate(core: &Core) -> Result<(), String> {
    match super::dry::blocks(&core.inner.deps.settings) {
        Some(why) => Err(format!("покупки нет: перед покупкой включён {why}")),
        None => Ok(()),
    }
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
    // Решение 66: покупка только с бюджетом подбора объёма — без него предела списания нет, сделки нет.
    let Some(budget) = plan.size.as_ref().map(|s| s.budget) else {
        let why = "в плане нет подбора объёма (бюджета) — покупка без предела списания не ставится";
        return super::ledger::finish(core, &mut deal, Step::Closed, why).await;
    };
    // Решение 74: сухой режим могли включить во время предпроверки — перед первым торговым вызовом ещё раз.
    if let Err(why) = buy_gate(core) {
        return super::ledger::finish(core, &mut deal, Step::Closed, &why).await;
    }
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
        Some(budget),
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
