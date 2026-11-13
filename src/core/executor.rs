//! Файл: core/executor.rs
//! Что делает: сделка целиком (шаг 5 ТЗ): предпроверка → покупка на A → перевод на B → продажа на B.
//! После каждого шага результат пересчитывается по факту. Не исполнилась покупка — сделка закрыта
//! без монет (стартовое правило шлюза: отменить, больше не пробовать — повтор только по правилу).
//! Покупка исполнилась частично — решение 48 (partial.rs): больший итог из «довезти и продать» и
//! «продать обратно на A». Вывод не состоялся — монеты на A: продажа здесь по порогу (решение 220, relay_here.rs);
//! монет на A нет — остановка. Решение 249: биржа → биржа запрещена (venue.cex_cex / venue.cex_cex_deny сменились после
//! начала сделки, cex_link.rs) — перевода нет, продажа здесь (relay_here.rs). Продажа: выгодно
//! (итог не ниже нуля, решение 47) — продаём; иначе — режим застрявшей монеты (решение 37).
//! Решение 118 (Я-2): по одной монете — одна сделка (замок по адресу контракта, coin_key.rs): направления одной монеты
//! через разные площадки не покупают её параллельно сверх money.max_per_coin_usd.
//! Исход покупки или перевода неизвестен (OutcomeUnknown) — сделка остановлена до ручной проверки (Step::Halted), без
//! застрявшей монеты, продажи и повторного перевода.
//! Решение 103: свободный баланс A перед ордером меньше списания плана — объём подбирается заново под остаток (refit.rs).
//! Решение 262: лимит ордера покупки на бирже — худший уровень стакана плана (Calc::buy_worst; после refit — нового
//! плана), в хронологию — «Лимит покупки = худший уровень плана X (план avg Y)».
//! Сухой режим (решение 74, dry.rs) — второй замок на входе: сделка не начинается, даже если кандидата передали сюда
//! (первый замок — candidates.rs); предпроверка считает план в режиме Live (виртуального бюджета у сделки нет); перед
//! покупкой — третий замок (buy_gate). После покупки сделка доводится как обычно.
//! Решение 94: свободный баланс площадки A читается один раз — сразу после третьего замка, прямо перед ордером
//! покупки (limits::buy_cap); предел списания — меньшее из бюджета плана и свободного.
//! Решение 132: счёт (кошелёк) DEX сделки — по маппингу перед предпроверкой (dex_account.rs); нет строки — сделки нет;
//! маппинг поменяли во время предпроверки — сделки нет. Счёт записывается в сделку, все шаги идут им.
//! Решение 141: покупка исполнилась — фоновый расчёт «без потолка» с id сделки (uncapped_entry.rs; путь не ждёт).
//! Решение 147: отказ на входе (NotTradable, Busy) несёт точную причину (start_reason.rs) — в «Кандидаты» и журнал.
//! Решение 245, дополнение п. 2: момент решения (Calc.at прошедшего кандидата) — в учёт сделки (Deal::decided_at).
//! Решение 265: «Время по шагам» в хронологию сделки (step_timing.rs): от расчёта кандидата и от передачи исполнителю
//! до старта задачи, лимит выводов, маппинг, предпроверка, открытие сделки; шаги предпроверки и этапы пересчёта
//! (precheck.rs); свободный баланс, подбор под остаток (со своими этапами), снимок рынка, ордер покупки целиком и
//! каждый его HTTP-запрос (замер снаружи вызова шлюза — gateway/step_meter.rs), газ, учёт исполнения. Только при
//! log.step_timing = да (читается один раз в начале сделки); выключено — ни замера, ни строк.
//! Решение 266: включатели быстрой предпроверки precheck.fast_recalc и precheck.parallel читаются один раз в начале
//! сделки (accepted, precheck_fast.rs read) и передаются предпроверке вместе с объёмом кандидата (Calc.qty); так же —
//! precheck.networks_memory и precheck.balance_first. Часть 4 (precheck.balance_first = да, изменяет решение 94):
//! свободный баланс A прочитан предпроверкой перед расчётом (precheck_balance.rs) — здесь он повторно не читается и
//! объём под остаток не подбирается: предел списания ордера — min(бюджет плана, свободно) по прочитанному; биржа A
//! баланс не отдала — ордер уходит на посчитанный объём с пределом = бюджет (строка об этом — в хронологии).
//! Кто вызывает: core/candidates.rs (прошедший кандидат).
//! От чего зависит: precheck.rs, trade_ops.rs, transfers.rs, fills.rs, ledger.rs, relay_here.rs, sell.rs, deal.rs, remeasure.rs
//! (частичная покупка: продажа на A — пере-замер по порогу «итог ≥ 0», решение 247, дополнение 10.11, t78).
//! Настройки: через вызываемые модули.

use crate::core::types::{Direction, Side};
use crate::gateway::urgency::{self, Urgency};
use crate::storage::journal::Level;

use super::costs::Calc;
use super::deal::{Deal, Step};
use super::engine::{Core, lk, rd};
use super::step_timing::{Steps, note};

/// Решение 265: этап «Время по шагам» до покупки (строка хронологии).
const STAGE: &str = "исполнение до покупки";
use std::time::Instant;

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
    // Решение 245, дополнение п. 2: момент решения — время расчёта кандидата, прошедшего пороги (Calc.at).
    let decided = calc.at;
    // Решение 265: от передачи исполнителю до старта задачи сделки — шаг «Время по шагам (исполнение до покупки)».
    // Включатель log.step_timing — один раз на сделку (здесь); выключен — ни часов, ни обёрток, ни строк.
    let on = super::step_timing::enabled(&core.inner.deps.settings);
    let handed = on.then(Instant::now);
    // Решение 266: включатели быстрой предпроверки — один раз на сделку (здесь); объём кандидата — Calc.qty.
    let fast = super::precheck_fast::read(&core.inner.deps.settings, &calc);
    tokio::spawn(urgency::scope(Urgency::Trade, async move {
        run(&core, d.clone(), decided, (on, handed), fast).await;
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

/// Ведёт сделку до конца или до передачи в режим застрявшей монеты. Принимает: ядро, направление, момент решения
/// (Calc.at прошедшего кандидата — в учёт сделки, решение 245, дополнение п. 2), момент передачи исполнителю
/// (решение 265: включён ли замер по шагам и момент передачи — None, если выключен), режим быстрой предпроверки
/// (решение 266: объём кандидата и «одновременно» — precheck_fast.rs; всё выключено — как раньше).
async fn run(
    core: &Core,
    d: Direction,
    decided: chrono::DateTime<chrono::Utc>,
    (on, handed): (bool, Option<Instant>),
    fast: super::precheck_fast::Opts,
) {
    let deps = &core.inner.deps;
    // Решение 265: «Время по шагам» от положительного расчёта до исполненной покупки (step_timing.rs).
    let mut ts = Steps::new(on);
    ts.add(|| {
        let since = chrono::Utc::now() - decided;
        (
            "от расчёта кандидата до старта исполнителя".into(),
            since.num_milliseconds().into(),
        )
    });
    ts.mark(handed, || "от передачи исполнителю до старта задачи".into());
    // Решение 132: счёт DEX сделки — по маппингу сейчас; нет строки — сделки нет (биржа без кошелька с DEX не торгует).
    let pre = async {
        // Решение 180: лимит выводов биржи A — место занимается в момент решения; окно занято — сигнал пропускается.
        // Решение 211: ключ лимита «биржа и монета» — тикер этой биржи (так же, как в transfers.rs take).
        let at = ts.at();
        let tick = super::venue_ticker::ticker_on(core, &d.asset, &d.buy_on);
        let slot = super::withdraw_limit::reserve(core, &d.buy_on, &tick)?;
        ts.mark(at, || "место в лимите выводов".into());
        let at = ts.at();
        let acc = super::dex_account::of_direction(deps, &d).await?;
        ts.mark(at, || "кошелёк DEX по маппингу".into());
        let at = ts.at();
        let plan = super::precheck::run(core, &d, on, fast, acc.as_ref()).await?;
        ts.mark(at, || "предпроверка".into());
        // Маппинг поменяли во время предпроверки — предпроверка шла другим кошельком: сделки нет.
        let at = ts.at();
        let same = super::dex_account::of_direction(deps, &d).await? == acc;
        ts.mark(at, || "маппинг повторно".into());
        if !same {
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
    let at = ts.at();
    let (plan, lines, bal) = plan;
    let parts = super::limits_parts::parts(core, &d, &super::dry::Mode::Live).ok();
    // Решение 118 (Я-17): ID сделки выдаётся и занимается в учёте под одним замком — повтор невозможен.
    let mut deal = super::ledger::open(
        core,
        &d,
        serde_json::to_value(&plan).unwrap_or_default(),
        plan.profit_usd,
        acc.clone(),
    );
    // Решение 245, дополнение п. 2: момент решения о сделке — в учёт (длительность сделки для «успели бы»).
    deal.decided_at = Some(decided);
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
    // Решение 265: время шагов предпроверки и этапов пересчёта — в хронологию (до сохранения; выключено — строк нет).
    // Решение 266: первой — строка «План: …» быстрого пересчёта (только при precheck.fast_recalc = да).
    for l in lines {
        deal.note(l);
    }
    super::ledger::save(core, &deal).await;
    ts.mark(at, || "открытие сделки в учёте".into());
    // Решение 66: покупка только с бюджетом подбора объёма — без него предела списания нет, сделки нет.
    let Some(budget) = plan.size.as_ref().map(|s| s.budget) else {
        let why = "в плане нет подбора объёма (бюджета) — покупка без предела списания не ставится";
        note(&mut deal, ts.text(STAGE));
        return super::ledger::finish(core, &mut deal, Step::Closed, why).await;
    };
    // Решение 74: сухой режим могли включить во время предпроверки — перед первым торговым вызовом ещё раз.
    if let Err(why) = buy_gate(core) {
        note(&mut deal, ts.text(STAGE));
        return super::ledger::finish(core, &mut deal, Step::Closed, &why).await;
    }
    // Решение 94: свободный баланс A — один запрос прямо перед ордером (не на каждый расчёт); предел списания —
    // меньшее из бюджета плана и свободного. Не хватает на объём плана — объём заново под остаток (решение 103).
    // Решение 266, часть 4 (precheck.balance_first = да): баланс уже прочитан предпроверкой перед расчётом (bal) —
    // здесь запроса нет; заметка — тем же текстом; биржа баланс не отдала — предел = бюджет, строка об этом.
    use super::precheck_balance::Read;
    use rust_decimal::Decimal;
    let before = |free: Decimal, q: &str, cap: Decimal| {
        format!(
            "Перед покупкой свободно на {}: {free} {q}; предел списания {cap}",
            d.buy_on.0
        )
    };
    let refit_note = |cap: Decimal, qty: Decimal, was: Decimal, profit: Decimal, old: Decimal| {
        format!(
            "Объём под свободный остаток {cap}: {qty} вместо {was}, прибыль {} USDT (было {})",
            profit.round_dp(4),
            old.round_dp(4)
        )
    };
    let cap = match bal.as_ref().map(|b| &b.read) {
        Some(Read::Got { free, quote }) => {
            let cap = budget.min(*free);
            deal.note(before(*free, quote, cap));
            cap
        }
        Some(Read::Missed(why)) => {
            deal.note(super::precheck_balance::missed_note(&d, why));
            budget
        }
        None => {
            let at = ts.at();
            let free = super::limits::buy_cap(core, &d, budget, deal.dex_account.as_ref()).await;
            ts.mark(at, || format!("свободный баланс {}", d.buy_on.0));
            match free {
                Ok((cap, free, q)) => {
                    deal.note(before(free, &q, cap));
                    cap
                }
                Err(why) => {
                    let why = format!("перед покупкой: {why} — ордер не ставился");
                    note(&mut deal, ts.text(STAGE));
                    return super::ledger::finish(core, &mut deal, Step::Closed, &why).await;
                }
            }
        }
    };
    // Часть 4: план уже посчитан под свободный остаток (свободного меньше списания кандидата) — та же заметка.
    if let Some(c) = bal.as_ref().and_then(|b| b.refit) {
        deal.note(refit_note(
            cap,
            plan.qty,
            c.qty,
            plan.profit_usd,
            c.profit_usd,
        ));
    }
    // Решение 103: свободного меньше списания плана — объём подбирается заново под свободный остаток (все затраты,
    // финальная котировка, пороги). Не выгодно — отказ до ордера. Часть 4 решения 266: подбор уже сделан предпроверкой.
    let plan = if bal.is_none() && super::refit::needed(&plan, cap) {
        // Решение 265: подбор под остаток — шаг и свои этапы пересчёта.
        let at = ts.at();
        let refit = Box::pin(super::refit::plan(core, &d, cap));
        let (r, m) = super::step_timing::measure_if(on, refit).await;
        ts.mark(at, || "подбор объёма под свободный остаток".into());
        note(
            &mut deal,
            m.map(|m| m.calc_text("подбор объёма под свободный остаток")),
        );
        match r {
            Ok(p) => {
                deal.note(refit_note(
                    cap,
                    p.qty,
                    plan.qty,
                    p.profit_usd,
                    plan.profit_usd,
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
                note(&mut deal, ts.text(STAGE));
                return super::ledger::finish(core, &mut deal, Step::Closed, &why).await;
            }
        }
    } else {
        plan
    };
    // Решение 217: рынок на момент решения — до ордера покупки (копии из памяти адаптеров; расчёт выключен — пропуск).
    let at = ts.at();
    let frozen = super::uncapped_entry::freeze(core, &plan).await;
    ts.mark(at, || "снимок рынка момента решения".into());
    // Покупка на A. Решение 262 (ТЗ «Покупка на A»): у биржи лимит IOC — худший уровень стакана плана (после refit —
    // нового плана), не текущий стакан; у DEX — как раньше (trade_ops.rs).
    if super::trade_ops::kind(core, &d.buy_on) == Ok(crate::gateway::contract::VenueKind::Cex) {
        deal.note(format!(
            "Лимит покупки = худший уровень плана {} (план avg {})",
            plan.buy_worst, plan.buy_quote.avg_price
        ));
    }
    let mut gas = Vec::new();
    let at = ts.at();
    // Будущее ордера — в куче (как в precheck.rs: обёртки замера не копируют его на стек).
    let (bought, order) = super::step_timing::measure_if(
        on,
        Box::pin(super::trade_ops::trade(
            core,
            &deal.id,
            &d.buy_on,
            &d.asset,
            Side::Buy,
            plan.qty,
            plan.buy_quote.route.clone(),
            plan.buy_quote.avg_price,
            Some(plan.buy_worst),
            // Решение 219: предел «количество × худшая цена» — за вычетом доли комиссии в котируемой монете.
            Some(plan.order_cap(cap)),
            Some(&mut gas),
        )),
    )
    .await;
    ts.mark(at, || format!("ордер покупки {}", d.buy_on.0));
    // Решение 236, дополнение: approve, чаевые, газ неудачных попыток — в затраты сделки (и при отказе покупки).
    let at = ts.at();
    super::deal_gas::charge(core, &mut deal, &d.buy_on, &mut gas).await;
    ts.mark(at, || "газ попыток в затраты сделки".into());
    // Решение 265: шаги до покупки и каждый запрос ордера (пробный, ордер, опросы статуса, сделки и комиссия).
    note(&mut deal, ts.text(STAGE));
    note(
        &mut deal,
        order.map(|o| o.reqs_text(&format!("ордер покупки {}", d.buy_on.0))),
    );
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
    let at = ts.at();
    super::fills::trade(core, &mut deal, &r).await;
    let mut after = Steps::new(on);
    after.mark(at, || "учёт исполнения покупки (комиссии в затраты)".into());
    note(&mut deal, after.text("после покупки"));
    deal.buy = Some(r.clone());
    // Решение 141: зашли в сделку — фоновый расчёт «без потолка» с id сделки (путь не ждёт); решение 217 — по снимку.
    // Часть 4 решения 266: бюджет плана уже урезан свободным балансом (расчёт шёл с пределом) — «бюджет плана до
    // чтения баланса» — меньшее из составляющих бюджета (parts, та же формула, что limits::budget); баланс не получен —
    // свободного остатка нет (None).
    let (plan_budget, free_cap) = match bal.as_ref().map(|b| &b.read) {
        None => (budget, Some(cap)),
        Some(Read::Got { free, .. }) => {
            let b = parts.as_ref().map_or(budget, |p| p.cap());
            (b, Some(b.min(*free)))
        }
        Some(Read::Missed(_)) => (budget, None),
    };
    super::uncapped_entry::deal(core, &deal.id, &plan, plan_budget, free_cap, parts, frozen);
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
                // Решение 247, дополнение 10.11 (t78, Тигран: согласовано): пере-замер продажи DEX при откате «мало на
                // выходе» — по порогу «итог сделки ≥ 0»; t81 (дополнение к 247 п. Г): PROFITABLE_OPEN — исполнить может и
                // агрегатор DEX, если он включён и его маршрут лучше.
                let g = super::remeasure::PROFITABLE_OPEN;
                return super::sell::market_guarded(core, deal, &why, g).await;
            }
            super::partial::Choice::Neither(e) => {
                let why = format!("частичная покупка: ни один путь не считается — {e}");
                return super::ledger::finish(core, &mut deal, Step::Halted, &why).await;
            }
        }
    }
    // Решение 249: биржа → биржа запрещена настройками, сменившимися после начала сделки, — на B не переводим:
    // продажа здесь по порогу stuck.relay_max_loss_pct (relay_here.rs), иначе решает человек.
    if let Err(e) = super::cex_link::direction_ok(core, &d) {
        deal.step = Step::Stuck;
        let why = format!("перевод на {} запрещён: {e}", d.sell_on.0);
        deal.note(format!("{why} — продажа здесь"));
        return super::relay_here::sell_here(core, deal, &why).await;
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
            // Решение 253 (г): отказ запоминается по паре «монета + биржа» — новые входы с неё закрыты.
            super::withdraw_block::close(core, &deal.id, &d.asset, &d.buy_on, &e).await;
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
