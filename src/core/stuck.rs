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
//! («Порог убытка» — stuck.max_loss_pct, «Ожидание: стало выгодно» — итог ≥ 0; по истечении ожидания — без пере-замера).
//! Кто вызывает: sell.rs, sell_unsent.rs, relay.rs (executor.rs — через sell.rs; перевод не состоялся — relay_here.rs, решение 220).
//! От чего зависит: sell.rs, trade_ops.rs, stuck_quote.rs, transfers.rs, fills.rs, manual.rs, relay.rs, ledger.rs, deal.rs, remain_fact.rs.
//! Настройки: stuck.mode, stuck.wait_ms, stuck.max_loss_pct, stuck.max_relays.

use std::time::Duration;

use tokio::time::Instant;

use crate::notify::human::{self as h, Msg};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::Core;
use super::remeasure::Guard;
use super::stuck_quote as q;
use super::stuck_reason::StuckReason;

/// Ведёт застрявшую монету по режиму. Принимает: ядро, сделку, настоящую причину, почему монета здесь (решение 220 —
/// в журнал, хронологию и Telegram). Решение 222: сначала остаток сверяется с балансом площадки (remain_fact.rs) —
/// дальше min(учёт, факт); на площадке ничего нет — сделка закрыта. Ошибок нет: не задан режим — сделка остановлена.
pub async fn handle(core: &Core, mut deal: Deal, why: StuckReason) {
    if !super::remain_fact::reconcile(core, &mut deal).await {
        return;
    }
    handle_as_is(core, deal, why).await
}

/// Режим застрявшей монеты без сверки с балансом — только после несостоявшегося перевода (executor.rs): монеты могут
/// быть в возврате вывода, баланс площадки ещё не факт (решение 222). Ошибок нет: не задан режим — сделка остановлена.
pub async fn handle_as_is(core: &Core, mut deal: Deal, why: StuckReason) {
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
    match mode.as_str() {
        "wait" => wait(core, deal).await,
        "loss_limit" => loss_limit(core, deal).await,
        "relay" => super::relay::run(core, deal).await,
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

/// Ждёт следующего тика монеты (или времени `until`). Возвращает: false — время вышло.
pub async fn next_tick(core: &Core, deal: &Deal, until: Option<Instant>) -> bool {
    let mut rx = core.inner.coin_ticks.subscribe();
    loop {
        let recv = rx.recv();
        let got = match until {
            Some(t) => match tokio::time::timeout_at(t, recv).await {
                Ok(r) => r,
                Err(_) => return false,
            },
            None => recv.await,
        };
        match got {
            Ok(a) if a == deal.direction.asset => return true,
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => return false,
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
    let until = Instant::now() + Duration::from_millis(ms);
    let mut first = true;
    while next_tick(core, &deal, Some(until)).await {
        match super::trade_ops::deal_quote(core, &deal).await {
            // Решение 47: выгодно — итог сделки не ниже нуля.
            Ok((v, _, _)) if super::sell::profitable(&deal, v) => {
                // Решение 247: пере-замер — по тому же условию «выгодно» (итог ≥ 0 — порог убытка 0).
                let g = Guard::MaxLoss(rust_decimal::Decimal::ZERO);
                let why = "ожидание: стало выгодно";
                return super::sell::market_guarded(core, deal, why, g).await;
            }
            Ok(_) => {}
            // Решение 238 (ревью, п. 2): не задана настройка — ждать нечего.
            Err(e) if q::not_configured(&e) => return q::halt_not_configured(core, deal, &e).await,
            Err(e) => q::no_quote_once(core, &deal, &e, &mut first).await,
        }
    }
    // Решение 247: по истечении ожидания — продажа по рынку без порога убытка, пере-замера нет (порога для него нет).
    let why = format!("ждали {ms} мс — выгодно не стало");
    sold_by_market(core, deal, &why, Guard::Off).await;
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
    let value = loop {
        match super::trade_ops::deal_quote(core, &deal).await {
            Ok((v, _, _)) => break v,
            Err(e) if q::not_configured(&e) => return q::halt_not_configured(core, deal, &e).await,
            Err(e) => {
                q::no_quote_once(core, &deal, &e, &mut first).await;
                if !next_tick(core, &deal, None).await {
                    return super::ledger::finish(core, &mut deal, Step::Halted, &e.message).await;
                }
            }
        }
    };
    let loss = deal.loss_pct_if(value);
    if loss <= max {
        return sold_by_market(
            core,
            deal,
            &h::loss_vs_max(loss, max, true),
            Guard::MaxLoss(max),
        )
        .await;
    }
    let why = h::loss_vs_max(loss, max, false);
    super::manual::raise_alert(core, deal, Some(loss), why, None).await;
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
