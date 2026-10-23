//! Файл: core/manual.rs
//! Что делает: ручное управление (решение 37): алерт «убыток выше порога» с кнопками «Продать сейчас»
//! и «Беру на себя»; «Беру на себя» — монеты списываются из учёта бота, бот работает дальше; «Вернуть
//! боту» — монеты возвращаются в учёт по реальному балансу площадки и продаются по общим правилам
//! (выгодно — сразу, иначе режим застрявшей монеты). Алерты и монеты человека хранятся в учёте ядра.
//! Кто вызывает: stuck.rs (raise_alert), commands.rs (команды), engine.rs (load при запуске).
//! От чего зависит: deal.rs, ledger.rs, sell.rs, persist.rs, view.rs, deps.rs (шлюз — балансы).
//! Настройки: нет.

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{ManualHolding, StuckAlert};
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};
use super::view::CoreEvent;

/// Публикует алерты и монеты человека в интерфейс.
pub fn publish(core: &Core) {
    let alerts = lk(&core.inner.alerts)
        .values()
        .map(|(a, _)| a.clone())
        .collect();
    let holdings = lk(&core.inner.holdings)
        .values()
        .map(|(h, _)| h.clone())
        .collect();
    core.inner.live.send(CoreEvent::Manual { alerts, holdings });
}

/// Сохраняет алерты и монеты человека (вместе со сделкой) в учёт ядра.
pub async fn persist(core: &Core) {
    let db = &core.inner.deps.db;
    let alerts: Vec<(StuckAlert, Deal)> = lk(&core.inner.alerts).values().cloned().collect();
    let holdings: Vec<(ManualHolding, Deal)> = lk(&core.inner.holdings).values().cloned().collect();
    let r1 = super::persist::save(db, "manual_alerts", "all", &alerts).await;
    let r2 = super::persist::save(db, "manual_holdings", "all", &holdings).await;
    if let Err(e) = r1.and(r2) {
        core.inner
            .deps
            .log(
                Level::Error,
                "core.manual",
                None,
                format!("Ручное управление не сохранено: {e}"),
                None,
            )
            .await;
    }
    publish(core);
}

/// Загружает алерты и монеты человека при запуске. Ошибка базы — в журнал.
pub async fn load(core: &Core) {
    let db = &core.inner.deps.db;
    let alerts = super::persist::load::<Vec<(StuckAlert, Deal)>>(db, "manual_alerts").await;
    let holdings = super::persist::load::<Vec<(ManualHolding, Deal)>>(db, "manual_holdings").await;
    match (alerts, holdings) {
        (Ok(a), Ok(h)) => {
            for (x, d) in a.into_iter().flatten() {
                lk(&core.inner.alerts).insert(x.id.clone(), (x, d));
            }
            for (x, d) in h.into_iter().flatten() {
                lk(&core.inner.holdings).insert(x.id.clone(), (x, d));
            }
        }
        (Err(e), _) | (_, Err(e)) => {
            core.inner
                .deps
                .log(
                    Level::Error,
                    "core.manual",
                    None,
                    format!("Ручное управление не загружено: {e}"),
                    None,
                )
                .await
        }
    }
    publish(core);
}

/// Алерт «убыток выше порога»: сделка ждёт решения человека. Уведомление stuck.loss_alert несёт id алерта
/// (кнопки Telegram — по notify/catalog.rs). Ошибок нет.
pub async fn raise_alert(core: &Core, mut deal: Deal, loss_pct: Decimal) {
    let id = match super::ids::uuid_v4() {
        Ok(id) => id,
        Err(e) => {
            let why = format!("алерт не создан ({e}) — монеты на месте, нужна ручная проверка");
            return super::ledger::finish(core, &mut deal, Step::Halted, &why).await;
        }
    };
    let alert = StuckAlert {
        id,
        deal_id: deal.id.clone(),
        venue: deal.at.clone(),
        asset: deal.direction.asset.clone(),
        qty: deal.qty,
        loss_pct,
        at: Utc::now(),
    };
    deal.step = Step::WaitingHuman;
    deal.note(format!(
        "Убыток {}% выше порога — ждём решения человека (алерт {})",
        loss_pct.round_dp(4),
        alert.id
    ));
    super::ledger::save(core, &deal).await;
    let text = format!(
        "{}: {} {} на {}, убыток при продаже сейчас {}%. Алерт {}",
        deal.id,
        deal.qty,
        alert.asset.ticker,
        alert.venue.0,
        loss_pct.round_dp(4),
        alert.id
    );
    lk(&core.inner.alerts).insert(alert.id.clone(), (alert.clone(), deal));
    persist(core).await;
    core.inner.live.send(CoreEvent::StuckAlert(alert.clone()));
    // Кнопки «Продать сейчас» и «Беру на себя» несут id алерта (notify/callback.rs).
    core.inner
        .deps
        .notifier
        .send_with_buttons("stuck.loss_alert", Some(&alert.venue.0), &text, &alert.id)
        .await;
}

/// «Продать сейчас» по алерту. Возвращает: текст для ответа. Ошибка: алерта нет.
pub async fn sell_now(core: &Core, alert_id: &str) -> Result<String, String> {
    let (_, deal) = lk(&core.inner.alerts)
        .remove(alert_id)
        .ok_or(format!("алерта {alert_id} нет"))?;
    persist(core).await;
    let id = deal.id.clone();
    let c = core.clone();
    tokio::spawn(async move {
        super::sell::market(&c, deal, "кнопка «Продать сейчас»").await
    });
    Ok(format!("Сделка {id}: продажа по рынку запущена"))
}

/// «Беру на себя»: монеты списываются из учёта бота. Возвращает: текст. Ошибка: алерта нет.
pub async fn take_over(core: &Core, alert_id: &str, who: &str) -> Result<String, String> {
    let (alert, mut deal) = lk(&core.inner.alerts)
        .remove(alert_id)
        .ok_or(format!("алерта {alert_id} нет"))?;
    let h = ManualHolding {
        id: super::ids::uuid_v4().unwrap_or_else(|_| alert.id.clone()),
        deal_id: deal.id.clone(),
        venue: alert.venue.clone(),
        asset: alert.asset.clone(),
        qty: deal.qty,
        reason: format!(
            "алерт {}: убыток {}% ({who})",
            alert.id,
            alert.loss_pct.round_dp(4)
        ),
        taken_at: Utc::now(),
    };
    super::ledger::finish(
        core,
        &mut deal,
        Step::TakenOver,
        "«Беру на себя» — монеты у человека",
    )
    .await;
    lk(&core.inner.holdings).insert(h.id.clone(), (h.clone(), deal));
    persist(core).await;
    let text = format!(
        "{} {} на {} — у человека ({})",
        h.qty, h.asset.ticker, h.venue.0, h.reason
    );
    core.inner
        .deps
        .notify("manual.taken", Some(&h.venue.0), &text)
        .await;
    Ok(text)
}
