//! Файл: core/stuck.rs
//! Что делает: застрявшая монета (решение 37) — монеты у бота, а продавать невыгодно. Три взаимоисключающих
//! режима (stuck.mode); «невыгодно» — итог сделки ниже нуля (решение 47): «Ожидание» — на каждый тик монеты проверяем, стало ли выгодно, не дольше
//! stuck.wait_ms, затем продажа по рынку; «Порог убытка» — убыток ≤ stuck.max_loss_pct — продажа по рынку,
//! больше — алерт с кнопками «Продать сейчас» и «Беру на себя» (manual.rs); «Перегон» — на площадку, где
//! лучше с учётом затрат перевода, не больше stuck.max_relays раз, затем продажа здесь с порогом
//! stuck.relay_max_loss_pct (решение 220, relay_here.rs).
//! Решение 220: причина входа (stuck_reason.rs) — настоящая, от вызывающего; в журнале и Telegram.
//! Решение 222: перед режимом остаток сверяется с балансом площадки (remain_fact.rs) — min(учёт, факт); после
//! несостоявшегося перевода — без сверки (handle_as_is).
//! Решение 238 (ревью, п. 2): котировка продажи отказала с кодом NotConfigured (не задана настройка — например,
//! venue.book_stale_ms площадки: стакана для ордера нет) — ждать тика бессмысленно, настройка сама не появится: в
//! «Ожидании», «Пороге убытка» и «Перегоне» (оценка «продать здесь») сделка сразу остановлена (Halted) с причиной
//! «не задана настройка …», журнал Error и уведомление закрытия сделки человеку (ledger::finish). Прочие ошибки
//! котировки — ждём тика, как раньше; первая — в журнал (Warn), чтобы ожидание было видно (stuck_quote.rs).
//! Решение 247: продажа режима — с пере-замером продажи DEX при откате по minOut в пределах порога режима
//! («Порог убытка» — stuck.max_loss_pct, «Ожидание: стало выгодно» — итог ≥ 0; по истечении ожидания — тоже итог ≥ 0:
//! дополнение 10.11, t78 — Тигран: согласовано; прежде без пере-замера).
//! Решение 415 (t200): котировки-решения «Ожидания» и «Порога убытка» — под ходом очереди продажи счёта только на время
//! котировки (sell_turn_fresh::once): не между котировкой и ордером другой продажи той же монеты на той же площадке и не
//! раньше её итога; сама продажа — sell.rs market_guarded (своя котировка и ордер под ходом).
//! Кто вызывает: sell.rs, sell_unsent.rs, relay.rs (executor.rs — через sell.rs; перевод не состоялся — relay_here.rs, решение 220).
//! От чего зависит: sell.rs, trade_ops.rs, sell_turn_fresh.rs, stuck_quote.rs, transfers.rs, fills.rs, manual.rs, relay.rs, ledger.rs, deal.rs, remain_fact.rs.
//! Решение 334: ожидание тика монеты — stuck_lag.rs Ticks (подписка одна на всё ожидание — ревью 333, Н4; отставание —
//! счёт, журнал и проверка сразу — решение 361).
//! Решение 417 (ревью t201, М-3): причина GasReserve — «Перегон» без продажи здесь при итоге ≥ 0 (relay::run_as, прежнее
//! поведение); ветки режима и продажа / алерт внутри них — в куче (Box::pin, стек теста core_exec_book_238).
//! «417 (ожидание)» (Тигран 19.11): продажи «Ожидания» (стало выгодно; истечение срока при итоге ≥ 0) и «Порога
//! убытка» — с перепроверкой условия на котировке под ходом очереди (sell_here_ok.rs try_sell_if): не выполнено —
//! ордера нет; «Ожидание» продолжается (истечение — по рынку, как раньше), «Порог убытка» — алерт (over_max).
//! Настройки: stuck.mode, stuck.wait_ms, stuck.max_loss_pct, stuck.max_relays.

use std::time::Duration;

use tokio::time::Instant;

use crate::notify::human::{self as h, Msg};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::Core;
use super::remeasure::Guard;
use super::sell_here_ok::{Cond, HereOk, How, Note};
use super::sell_range::Rule;
use super::stuck_lag::Ticks;
use super::stuck_quote as q;
use super::stuck_reason::StuckReason;

/// Ведёт застрявшую монету по режиму. Принимает: ядро, сделку, настоящую причину, почему монета здесь (решение 220 —
/// в журнал, хронологию и Telegram). Решение 222: сначала остаток сверяется с балансом площадки (remain_fact.rs) —
/// дальше min(учёт, факт); на площадке ничего нет — сделка закрыта. Ошибок нет: не задан режим — сделка остановлена.
pub async fn handle(core: &Core, mut deal: Deal, why: StuckReason) {
    // Третье ревью t198 (М-6): факт только что прочитан на входе в продажу (sell_fact::before, «второй вход подряд» без
    // ожидания) — не читаем снова; отметка одноразовая. Иначе — сверка остатка (решение 413 — с долями), в куче.
    if !super::sell_fact::fresh_take(&mut deal)
        && !Box::pin(super::remain_fact::reconcile(core, &mut deal)).await
    {
        return;
    }
    Box::pin(handle_as_is(core, deal, why)).await
}

/// Режим застрявшей монеты без сверки с балансом — только после несостоявшегося перевода (executor.rs): монеты могут
/// быть в возврате вывода, баланс площадки ещё не факт (решение 222). Ошибок нет: не задан режим — сделка остановлена.
pub async fn handle_as_is(core: &Core, mut deal: Deal, why: StuckReason) {
    // Решение 413 (третье ревью, Б-1): дальше режим ждёт (время, котировку) — свежесть факта не переносится.
    deal.fact_fresh = None;
    let s = &core.inner.deps.settings;
    let mode = match s.text("stuck.mode", At::default()) {
        Ok(m) => m,
        Err(e) => {
            return super::ledger::finish(core, &mut deal, Step::Halted, &e.message).await;
        }
    };
    deal.step = Step::Stuck;
    deal.note(format!("Застрявшая монета ({}): режим {mode}", why.text()));
    super::ledger::save(core, &deal).await;
    let text = format!(
        "{}: {} {} на {} — {}, режим {mode}",
        deal.id,
        deal.qty,
        deal.direction.asset.ticker,
        deal.at.0,
        why.text()
    );
    core.inner
        .deps
        .log(Level::Warn, "core.stuck", Some(&deal.at.0), &text, None)
        .await;
    // Решение 164: в Telegram — по-человечески (режим — словами, числа округлены).
    // Решение 220: заголовок и причина — настоящие (не всегда «невыгодно»).
    let mut tg = Msg::new(h::WARN, format!("Монета застряла: {}", why.title()))
        .card(super::deal_card::card(&deal));
    if why != StuckReason::Unprofitable {
        tg = tg.reason(&why.text());
    }
    let tg = tg
        .kv("Что бот делает", mode_name(&mode))
        .id(&deal.id)
        .render();
    core.inner
        .deps
        .notify("stuck.detected", Some(&deal.at.0), &tg)
        .await;
    // Варианты настройки — wait / loss_limit / relay (settings/catalog/trade.rs).
    // Решение 417 (CI t201, как 580e950f ветки feat/arbitrum): ветки режима — в куче (Box::pin): их будущие (продажа,
    // перегон, алерт) крупные и вложены в будущее handle_as_is целиком — тест core_exec_book_238 переполнял стек.
    // Ревью t201 (М-3): GasReserve (BNB на DEX, газ продажи не помещается сверх резерва) — «Перегон» без продажи здесь
    // при итоге ≥ 0 (иначе быстрый круг «режим → продажа → газ не помещается → режим»), прежнее поведение.
    let profit_first = !matches!(why, StuckReason::GasReserve(_));
    match mode.as_str() {
        "wait" => Box::pin(wait(core, deal)).await,
        "loss_limit" => Box::pin(loss_limit(core, deal)).await,
        "relay" => Box::pin(super::relay::run_as(core, deal, profit_first)).await,
        other => {
            super::ledger::finish(
                core,
                &mut deal,
                Step::Halted,
                &format!("неизвестный режим stuck.mode = {other}"),
            )
            .await
        }
    }
}

/// «Ожидание»: стало выгодно — продаём; время вышло — по рынку.
async fn wait(core: &Core, mut deal: Deal) {
    let ms = match core
        .inner
        .deps
        .settings
        .integer("stuck.wait_ms", At::default())
    {
        Ok(v) => v,
        Err(e) => return super::ledger::finish(core, &mut deal, Step::Halted, &e.message).await,
    };
    // Ревью 333, Н4: подписка на тики — одна на всё ожидание: тик монеты, пришедший во время котировки, не теряется.
    let mut ticks = Ticks::new(core);
    let until = Instant::now() + Duration::from_millis(ms);
    let mut first = true;
    while ticks.next(core, &deal, Some(until)).await {
        match Box::pin(super::sell_turn_fresh::once(core, &deal)).await {
            // Решение 47: выгодно — итог сделки не ниже нуля.
            Ok((v, _, _)) if super::sell::profitable(&deal, v) => {
                // Решение 247: пере-замер — по тому же условию «выгодно» (итог ≥ 0 — порог убытка 0).
                // «417 (ожидание)»: условие перепроверяется на котировке под ходом очереди (sell_here_ok.rs); под ходом
                // итог < 0 или котировки нет — ордера нет, ожидание продолжается (срок вышел — правила истечения ниже).
                let zero = rust_decimal::Decimal::ZERO;
                let how = How {
                    cond: Cond::Profitable,
                    guard: Guard::MaxLoss(zero),
                    rule: Rule::MaxLoss(zero),
                    note: Note::None,
                };
                let why = "ожидание: стало выгодно";
                match Box::pin(super::sell_here_ok::try_sell_if(core, deal, why, how)).await {
                    HereOk::NotNow(d, _) => deal = *d,
                    HereOk::Sold | HereOk::Gone => return,
                }
            }
            Ok(_) => {}
            // Решение 238 (ревью, п. 2): не задана настройка — ждать нечего.
            Err(e) if q::not_configured(&e) => return q::halt_not_configured(core, deal, &e).await,
            Err(e) => q::no_quote_once(core, &deal, &e, &mut first).await,
        }
    }
    // Решение 247, дополнение 10.11 (t78, Тигран: согласовано): по истечении ожидания — продажа по рынку без порога
    // убытка; пере-замер продажи DEX при откате «мало на выходе» — только при итоге сделки ≥ 0. t81 (дополнение к 247
    // п. Г, Тигран 10.11: согласовано): PROFITABLE_OPEN — исполнить может и агрегатор DEX, если он включён и лучше.
    let why = format!("ждали {ms} мс — выгодно не стало");
    let g = super::remeasure::PROFITABLE_OPEN;
    // «417 (ожидание)»: под ходом итог ≥ 0 — продажа (правило Тиграна); иначе — как раньше по истечении срока: по рынку
    // без порога (котировка ордера — под ходом, market_guarded).
    let how = How {
        cond: Cond::Profitable,
        guard: g,
        rule: Rule::NoThreshold,
        note: Note::ProfitHere,
    };
    let deal = match Box::pin(super::sell_here_ok::try_sell_if(core, deal, &why, how)).await {
        HereOk::NotNow(d, _) => *d,
        HereOk::Sold | HereOk::Gone => return,
    };
    Box::pin(sold_by_market(core, deal, &why, g)).await;
}

/// «Порог убытка»: убыток не больше stuck.max_loss_pct — по рынку; больше — алерт человеку.
async fn loss_limit(core: &Core, mut deal: Deal) {
    let max = match core
        .inner
        .deps
        .settings
        .decimal("stuck.max_loss_pct", At::default())
    {
        Ok(v) => v,
        Err(e) => return super::ledger::finish(core, &mut deal, Step::Halted, &e.message).await,
    };
    // Нет котировки — ждём тика монеты: оценивать убыток вслепую нельзя. Решение 238 (ревью, п. 2): не задана
    // настройка (NotConfigured) — не ждём (без предела ждали бы вечно без алерта), сделка остановлена.
    let mut first = true;
    // Ревью 333, Н4: подписка на тики — до первой котировки и на всё ожидание (тик во время котировки не теряется).
    let mut ticks = Ticks::new(core);
    let value = loop {
        match Box::pin(super::sell_turn_fresh::once(core, &deal)).await {
            Ok((v, _, _)) => break v,
            Err(e) if q::not_configured(&e) => return q::halt_not_configured(core, deal, &e).await,
            Err(e) => {
                q::no_quote_once(core, &deal, &e, &mut first).await;
                if !ticks.next(core, &deal, None).await {
                    return super::ledger::finish(core, &mut deal, Step::Halted, &e.message).await;
                }
            }
        }
    };
    let loss = deal.loss_pct_if(value);
    if loss <= max {
        let text = h::loss_vs_max(loss, max, true);
        // «417 (ожидание)»: порог перепроверяется на котировке под ходом очереди (sell_here_ok.rs); выше порога или
        // котировки нет — алерт человеку, как «больше — алерт» (убыток — по котировке под ходом).
        let how = How {
            cond: Cond::MaxLoss(max),
            guard: Guard::MaxLoss(max),
            rule: Rule::MaxLoss(max),
            note: Note::StuckMarket,
        };
        let (d, v) = match Box::pin(super::sell_here_ok::try_sell_if(core, deal, &text, how)).await
        {
            HereOk::NotNow(d, v) => (*d, v),
            HereOk::Sold | HereOk::Gone => return,
        };
        let (loss, why) = over_max(&d, v, max);
        return Box::pin(super::manual::raise_alert(core, d, loss, why, None)).await;
    }
    let why = h::loss_vs_max(loss, max, false);
    Box::pin(super::manual::raise_alert(
        core,
        deal,
        Some(loss),
        why,
        None,
    ))
    .await;
}

/// «417 (ожидание)»: порог не выполнен на котировке под ходом — убыток и причина алерта. Принимает: сделку, выручку
/// под ходом (None — котировки нет), порог. Возвращает: (убыток, текст). Ошибок нет.
pub fn over_max(
    deal: &Deal,
    v: Option<rust_decimal::Decimal>,
    max: rust_decimal::Decimal,
) -> (Option<rust_decimal::Decimal>, String) {
    match v {
        Some(v) => {
            let loss = deal.loss_pct_if(v);
            (Some(loss), h::loss_vs_max(loss, max, false))
        }
        None => (
            None,
            "котировки продажи под ходом очереди нет — убыток неизвестен, по рынку не продаём"
                .to_string(),
        ),
    }
}

/// Продажа по рынку с уведомлением stuck.market_sold. Решение 247: `g` — порог, по которому решено продать (граница
/// пере-замера продажи DEX при откате по minOut, core/remeasure.rs; Guard::Off — без пере-замера).
pub async fn sold_by_market(core: &Core, deal: Deal, why: &str, g: Guard) {
    let text = Msg::new(h::WARN, "Застрявшая монета продаётся по рынку")
        .card(super::deal_card::card(&deal))
        .reason(why)
        .id(&deal.id)
        .render();
    core.inner
        .deps
        .notify("stuck.market_sold", Some(&deal.at.0), &text)
        .await;
    super::sell::market_guarded(core, deal, why, g).await;
}

/// Режим застрявшей монеты словами (варианты stuck.mode — settings/catalog/trade.rs). Принимает: значение настройки.
fn mode_name(mode: &str) -> String {
    match mode {
        "wait" => "ждёт, пока станет выгодно".into(),
        "loss_limit" => "продаст, если убыток не выше порога, иначе спросит вас".into(),
        "relay" => "ищет площадку, где продать выгоднее".into(),
        other => other.to_string(),
    }
}
