//! Файл: core/executor.rs
//! Что делает: сделка целиком (шаг 5 ТЗ): предпроверка → покупка на A → перевод на B → продажа на B.
//! После каждого шага результат пересчитывается по факту. Не исполнилась покупка — сделка закрыта
//! без монет (стартовое правило шлюза: отменить, больше не пробовать — повтор только по правилу).
//! Покупка исполнилась частично — решение 48 (partial.rs): больший итог из «довезти и продать» и
//! «продать обратно на A». Вывод не состоялся — монеты на A: продажа здесь по порогу (решение 220, relay_here.rs);
//! монет на A нет — остановка. Продажа: выгодно
//! (итог не ниже нуля, решение 47) — продаём; иначе — режим застрявшей монеты (решение 37).
//! Решение 118 (Я-2): по одной монете — одна сделка (замок по адресу контракта, coin_key.rs): направления одной монеты
//! через разные площадки не покупают её параллельно сверх money.max_per_coin_usd.
//! Исход покупки или перевода неизвестен (OutcomeUnknown) — сделка остановлена до ручной проверки (Step::Halted), без
//! застрявшей монеты, продажи и повторного перевода.
//! Решение 103: свободный баланс A перед ордером меньше списания плана — объём подбирается заново под остаток (refit.rs).
//! Сухой режим (решение 74, dry.rs) — второй замок на входе: сделка не начинается, даже если кандидата передали сюда
//! (первый замок — candidates.rs); предпроверка считает план в режиме Live (виртуального бюджета у сделки нет); перед
//! покупкой — третий замок (buy_gate). После покупки сделка доводится как обычно.
//! Решение 94: свободный баланс площадки A читается один раз — сразу после третьего замка, прямо перед ордером
//! покупки (limits::buy_cap); предел списания — меньшее из бюджета плана и свободного.
//! Решение 132: счёт (кошелёк) DEX сделки — по маппингу перед предпроверкой (dex_account.rs); нет строки — сделки нет;
//! маппинг поменяли во время предпроверки — сделки нет. Счёт записывается в сделку, все шаги идут им.
//! Решение 141: покупка исполнилась — фоновый расчёт «без потолка» с id сделки (uncapped_entry.rs; путь не ждёт).
//! Решение 147: отказ на входе (NotTradable, Busy) несёт точную причину (start_reason.rs) — в «Кандидаты» и журнал.
//! Кто вызывает: core/candidates.rs (прошедший кандидат).
//! От чего зависит: precheck.rs, trade_ops.rs, transfers.rs, fills.rs, ledger.rs, relay_here.rs, sell.rs, deal.rs.
//! Настройки: через вызываемые модули.

use crate::core::types::{Direction, Side};
use crate::gateway::urgency::{self, Urgency};
use crate::storage::journal::Level;

use super::costs::Calc;
use super::deal::{Deal, Step};
use super::engine::{Core, lk, rd};

/// Что вышло на входе исполнителя.
#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    /// Сделка запущена.
    Started,
    /// Сухой режим (решение 74): ничего не делается — причина.
    Dry(String),
    /// Направление больше не торгуемо (чёрный список, налоги не проверены, монеты нет в справочнике, площадка
    /// исключена) — точная причина (решение 147, start_reason::not_tradable).
    NotTradable(String),
    /// По монете уже идёт сделка или действие (решение 118: одна сделка на монету) — какое (решение 147).
    Busy(String),
}

/// Кандидат прошёл расчёт — запускает сделку, если режим не сухой (решение 74), направление по-прежнему торгуемо
/// (есть в directions() монеты: за время расчёта монета могла уйти в чёрный список или в «налоги не проверены») и
/// по этой монете сейчас нет другой сделки (решение 118, Я-2: замок по монете, а не по направлению). Вся сделка — класс срочности Trade (gateway/urgency.rs, решение 61).
/// Возвращает: Start — что вышло. Ошибок нет.
pub async fn accepted(core: &Core, calc: Calc) -> Start {
    // Сухой режим проверяется первым: до справочника, учёта и шлюза — ни одного действия.
    if let Some(why) = super::dry::blocks(&core.inner.deps.settings) {
        return Start::Dry(why);
    }
    let d = calc.direction.clone();
    let refused = {
        let coins = rd(&core.inner.coins);
        let c = super::ticker_index::find(core, &coins, &d.asset);
        let ok = c.is_some_and(|c| c.directions().contains(&d));
        // Решение 147: причина — по той же записи справочника, по которой решено.
        (!ok).then(|| super::start_reason::not_tradable(c, &d))
    };
    if let Some(why) = refused {
        return Start::NotTradable(why);
    }
    let key = super::coin_key::CoinKey::of(&d.asset);
    if !lk(&core.inner.active).insert(key.clone()) {
        return Start::Busy(super::start_reason::busy(
            lk(&core.inner.deals).values(),
            &d,
        ));
    }
    let core = core.clone();
    tokio::spawn(urgency::scope(Urgency::Trade, async move {
        run(&core, d.clone()).await;
        lk(&core.inner.active).remove(&key);
    }));
    Start::Started
}

/// Замок перед покупкой (решение 74): сухой режим включён — покупки нет. Ошибки: текст причины.
pub fn buy_gate(core: &Core) -> Result<(), String> {
    match super::dry::blocks(&core.inner.deps.settings) {
        Some(why) => Err(format!(
            "остановлена: включён сухой режим до покупки — ордер не ставился ({why})"
        )),
        None => Ok(()),
    }
}

/// Ведёт сделку до конца или до передачи в режим застрявшей монеты.
async fn run(core: &Core, d: Direction) {
    let deps = &core.inner.deps;
    // Решение 132: счёт DEX сделки — по маппингу сейчас; нет строки — сделки нет (биржа без кошелька с DEX не торгует).
    let pre = async {
        // Решение 180: лимит выводов биржи A — место занимается в момент решения; окно занято — сигнал пропускается.
        // Решение 211: ключ лимита «биржа и монета» — тикер этой биржи (так же, как в transfers.rs take).
        let on = super::venue_ticker::ticker_on(core, &d.asset, &d.buy_on);
        let slot = super::withdraw_limit::reserve(core, &d.buy_on, &on)?;
        let acc = super::dex_account::of_direction(deps, &d).await?;
        let plan = super::precheck::run(core, &d).await?;
        // Маппинг поменяли во время предпроверки — предпроверка шла другим кошельком: сделки нет.
        if super::dex_account::of_direction(deps, &d).await? != acc {
            return Err("маппинг кошельков изменился во время предпроверки".to_string());
        }
        Ok((acc, plan, slot))
    };
    // Решение 155: отказ предпроверки повторяется на каждый прошедший расчёт (в тексте — новые миллисекунды) — одна
    // строка журнала на вид причины со счётчиком, как у core.candidate_rejected; прошла — строка закрывается.
    let key = format!("precheck|{}", super::candidates::reject_key(&d));
    let (acc, plan, slot) = match pre.await {
        Ok(x) => {
            core.inner.last_reject.close(&deps.journal, &key).await;
            x
        }
        Err(e) => {
            let message = format!(
                "{} {} → {}: сделки нет — {e}",
                d.asset.ticker, d.buy_on.0, d.sell_on.0
            );
            let note = super::reject_log::Note {
                level: Level::Info,
                kind: "core.precheck",
                venue: Some(&d.buy_on.0),
                shape: super::reject_shape::shape(&e),
                exact: e.clone(),
                message,
                data: None,
            };
            let every = super::candidates::flush_every(core);
            core.inner
                .last_reject
                .note(&deps.journal, &key, note, every)
                .await;
            return;
        }
    };
    // Решение 141: составляющие бюджета — до открытия сделки (своя сделка ещё не в учёте лимитов).
    let parts = super::limits_parts::parts(core, &d, &super::dry::Mode::Live).ok();
    // Решение 118 (Я-17): ID сделки выдаётся и занимается в учёте под одним замком — повтор невозможен.
    let mut deal = super::ledger::open(
        core,
        &d,
        serde_json::to_value(&plan).unwrap_or_default(),
        plan.profit_usd,
        acc.clone(),
    );
    // Решение 180: место в окне выводов — за сделкой до её вывода (transfers.rs) или конца (ledger.rs).
    if let Some(s) = slot {
        super::withdraw_limit::bind(core, &deal.id, s);
    }
    // Решение 123: выбор исполнителя DEX по финальной котировке («лучший из двух») — в хронологию.
    deal.note(format!(
        "План: объём {}, прибыль {} USDT{}",
        plan.qty,
        plan.profit_usd.round_dp(4),
        plan.pick_note()
    ));
    if let Some(a) = &acc {
        let name = deps.settings.accounts().wallet(&a.0).map(|w| w.name);
        deal.note(format!("Кошелёк DEX: «{}» ({a})", name.unwrap_or_default()));
    }
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
    // Решение 94: свободный баланс A — один запрос прямо перед ордером (не на каждый расчёт); предел списания —
    // меньшее из бюджета плана и свободного. Не хватает на объём плана — объём заново под остаток (решение 103).
    let cap = match super::limits::buy_cap(core, &d, budget, deal.dex_account.as_ref()).await {
        Ok((cap, free, q)) => {
            deal.note(format!(
                "Перед покупкой свободно на {}: {free} {q}; предел списания {cap}",
                d.buy_on.0
            ));
            cap
        }
        Err(why) => {
            let why = format!("перед покупкой: {why} — ордер не ставился");
            return super::ledger::finish(core, &mut deal, Step::Closed, &why).await;
        }
    };
    // Решение 103: свободного меньше списания плана — объём подбирается заново под свободный остаток (все затраты,
    // финальная котировка, пороги). Не выгодно — отказ до ордера.
    let plan = if super::refit::needed(&plan, cap) {
        match super::refit::plan(core, &d, cap).await {
            Ok(p) => {
                deal.note(format!(
                    "Объём под свободный остаток {cap}: {} вместо {}, прибыль {} USDT (было {})",
                    p.qty,
                    plan.qty,
                    p.profit_usd.round_dp(4),
                    plan.profit_usd.round_dp(4)
                ));
                deal.plan = serde_json::to_value(&p).unwrap_or_default();
                deal.plan_profit_usd = p.profit_usd;
                super::ledger::save(core, &deal).await;
                p
            }
            Err(why) => {
                let why = format!(
                    "на свободный остаток {cap} сделка невыгодна: {why} — ордер не ставился"
                );
                return super::ledger::finish(core, &mut deal, Step::Closed, &why).await;
            }
        }
    } else {
        plan
    };
    // Решение 217: рынок на момент решения — до ордера покупки (копии из памяти адаптеров; расчёт выключен — пропуск).
    let frozen = super::uncapped_entry::freeze(core, &plan).await;
    // Покупка на A.
    let mut gas = Vec::new();
    let bought = super::trade_ops::trade(
        core,
        &deal.id,
        &d.buy_on,
        &d.asset,
        Side::Buy,
        plan.qty,
        plan.buy_quote.route.clone(),
        plan.buy_quote.avg_price,
        // Решение 219: предел «количество × худшая цена» — за вычетом доли комиссии в котируемой монете.
        Some(plan.order_cap(cap)),
        Some(&mut gas),
    )
    .await;
    // Решение 236, дополнение: approve, чаевые, газ неудачных попыток — в затраты сделки (и при отказе покупки).
    super::deal_gas::charge(core, &mut deal, &d.buy_on, &mut gas).await;
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
    // Решение 141: зашли в сделку — фоновый расчёт «без потолка» с id сделки (путь не ждёт); решение 217 — по снимку.
    super::uncapped_entry::deal(core, &deal.id, &plan, budget, Some(cap), parts, frozen);
    let text = crate::notify::human::Msg::new(crate::notify::human::OK, "Куплено")
        .line(format!(
            "{} {} на {} по {}",
            crate::notify::human::qty(r.filled_qty),
            d.asset.ticker,
            crate::notify::human::venue(&d.buy_on.0),
            crate::notify::human::qty(r.avg_price)
        ))
        .kv(
            "Сумма",
            crate::notify::human::usd(r.filled_qty * r.avg_price),
        )
        .kv(
            "План",
            crate::notify::human::signed_usd(deal.plan_profit_usd),
        )
        .id(&deal.id)
        .render();
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
    // Решение 151: на кошелёк DEX — зачисление и по приходу в блокчейне (transfers::send_trade).
    let mut gas = Vec::new();
    let sent = super::transfers::send_trade(
        core,
        &deal.id,
        (&d.buy_on, &d.sell_on),
        &d.asset,
        deal.qty,
        &mut gas,
    )
    .await;
    // Решение 236, дополнение: газ неудачных переводов из кошелька DEX (откат, отмена) — в затраты сделки.
    super::deal_gas::charge(core, &mut deal, &d.buy_on, &mut gas).await;
    match sent {
        Ok((sent, credited, chain)) => {
            if let Some(line) = chain {
                deal.note(line);
            }
            super::fills::transfer(core, &mut deal, &sent, &credited, &d.sell_on, r.avg_price).await
        }
        Err(e) if e.outcome_unknown() => return unknown(core, &mut deal, "перевод", &e).await,
        Err(e) => {
            deal.step = Step::Stuck;
            deal.note(format!("Перевод не состоялся: {e}"));
            // Решение 227: отказ вывода площадкой — метка, перегон с неё больше не выбирается (relay.rs).
            super::withdraw_refused::mark_at(core, &mut deal, &d.buy_on, &e);
            // Решение 220 (Тигран 08.11, п. 2): монеты на A — продажа здесь по порогу stuck.relay_max_loss_pct
            // (relay_here.rs), без повтора вывода; монет на A нет — остановка, решает человек.
            let why = format!("перевод не состоялся: {e}");
            return super::relay_here::transfer_failed(core, deal, &why).await;
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
