//! Файл: core/stuck.rs
//! Что делает: застрявшая монета (решение 37) — монеты у бота, а продавать невыгодно. Три взаимоисключающих
//! режима (stuck.mode); «невыгодно» — итог сделки ниже нуля (решение 47): «Ожидание» — на каждый тик монеты проверяем, стало ли выгодно, не дольше
//! stuck.wait_ms, затем продажа по рынку; «Порог убытка» — убыток ≤ stuck.max_loss_pct — продажа по рынку,
//! больше — алерт с кнопками «Продать сейчас» и «Беру на себя» (manual.rs); «Перегон» — на площадку, где
//! лучше с учётом затрат перевода, не больше stuck.max_relays раз, затем продажа по рынку.
//! Кто вызывает: executor.rs, sell.rs.
//! От чего зависит: sell.rs, trade_ops.rs, transfers.rs, fills.rs, manual.rs, relay.rs, ledger.rs, deal.rs.
//! Настройки: stuck.mode, stuck.wait_ms, stuck.max_loss_pct, stuck.max_relays.

use std::time::Duration;

use tokio::time::Instant;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::Core;

/// Ведёт застрявшую монету по режиму. Ошибок нет: не задан режим — сделка остановлена с причиной.
pub async fn handle(core: &Core, mut deal: Deal) {
    let s = &core.inner.deps.settings;
    let mode = match s.text("stuck.mode", At::default()) {
        Ok(m) => m,
        Err(e) => {
            return super::ledger::finish(core, &mut deal, Step::Halted, &e.message).await;
        }
    };
    deal.step = Step::Stuck;
    deal.note(format!("Застрявшая монета: режим {mode}"));
    super::ledger::save(core, &deal).await;
    let text = format!(
        "{}: {} {} на {} — продавать невыгодно, режим {mode}",
        deal.id, deal.qty, deal.direction.asset.ticker, deal.at.0
    );
    core.inner
        .deps
        .log(Level::Warn, "core.stuck", Some(&deal.at.0), &text, None)
        .await;
    core.inner
        .deps
        .notify("stuck.detected", Some(&deal.at.0), &text)
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
    while next_tick(core, &deal, Some(until)).await {
        let v = super::trade_ops::sell_value(core, &deal.at, &deal.direction.asset, deal.qty).await;
        // Решение 47: выгодно — итог сделки не ниже нуля.
        if let Ok((v, _)) = v
            && super::sell::profitable(&deal, v)
        {
            return super::sell::market(core, deal, "ожидание: стало выгодно").await;
        }
    }
    sold_by_market(core, deal, &format!("ожидание {ms} мс истекло")).await;
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
    // Нет котировки — ждём тика монеты: оценивать убыток вслепую нельзя.
    let value = loop {
        match super::trade_ops::sell_value(core, &deal.at, &deal.direction.asset, deal.qty).await {
            Ok((v, _)) => break v,
            Err(_) if next_tick(core, &deal, None).await => continue,
            Err(e) => return super::ledger::finish(core, &mut deal, Step::Halted, &e).await,
        }
    };
    let loss = deal.loss_pct_if(value);
    if loss <= max {
        return sold_by_market(
            core,
            deal,
            &format!("убыток {}% ≤ порога {max}%", loss.round_dp(4)),
        )
        .await;
    }
    super::manual::raise_alert(core, deal, loss).await;
}

/// Продажа по рынку с уведомлением stuck.market_sold.
pub async fn sold_by_market(core: &Core, deal: Deal, why: &str) {
    let text = format!(
        "{}: {} {} на {} — продажа по рынку: {why}",
        deal.id, deal.qty, deal.direction.asset.ticker, deal.at.0
    );
    core.inner
        .deps
        .notify("stuck.market_sold", Some(&deal.at.0), &text)
        .await;
    super::sell::market(core, deal, why).await;
}
