//! Файл: core/credit_mismatch.rs
//! Что делает: сверка зачисления по событию и по статусу после того, как перевод уже зачислен (решение 76):
//! - зачислено по событию — опрос deposit_status продолжается до окончательного статуса (подтверждение; у DEX он же
//!   снимает ожидание поступления в кошелёк, wallet/inflows.rs); не «зачислен» за transfer.credit_mismatch_s или
//!   «отклонён» — журнал;
//! - зачислено по статусу — ожидание держится ещё transfer.credit_mismatch_s, чтобы позднее событие сопоставилось, а
//!   не стало «внешним»; события нет за окно (у площадки, приславшей хоть одно событие баланса) — журнал.
//!
//! Записи — вид core.credit_mismatch, уведомление exec.credit_mismatch.
//!
//! Кто вызывает: core/transfer_credit.rs.
//! От чего зависит: credit_watch.rs (Ticket), wait.rs, deps.rs (шлюз, журнал), settings.
//! Настройки: transfer.credit_mismatch_s, transfer.max_time_s, venue.status_poll_ms (по площадке получателя).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::core::types::{Asset, TransferStatus, VenueId};
use crate::gateway::contract::Venue;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::credit_watch::Ticket;
use super::engine::Core;
use super::transfer_credit::Leg;
use super::wait::{Pacer, Unit};

/// Данные шага без заимствований — для фоновой сверки.
pub struct Owned {
    deal: String,
    to: VenueId,
    asset: Asset,
    tx: String,
    started: chrono::DateTime<chrono::Utc>,
}

/// Данные шага для фоновой сверки. Ошибок нет.
pub fn owned(leg: &Leg<'_>) -> Owned {
    Owned {
        deal: leg.deal.to_string(),
        to: leg.to.clone(),
        asset: leg.asset.clone(),
        tx: leg.tx.clone(),
        started: leg.started,
    }
}

/// Предел сверки transfer.credit_mismatch_s. None — не задана: сверки нет (журнал один раз на перевод).
async fn limit(core: &Core, o: &Owned) -> Option<Duration> {
    let at = At {
        venue: Some(&o.to.0),
        coin: Some(&o.asset.ticker),
    };
    match core
        .inner
        .deps
        .settings
        .integer("transfer.credit_mismatch_s", at)
    {
        Ok(s) => Some(Duration::from_secs(s)),
        Err(e) => {
            let text = format!("{}: сверки события и статуса нет — {}", o.deal, e.message);
            core.inner
                .deps
                .log(
                    Level::Warn,
                    "core.credit_mismatch",
                    Some(&o.to.0),
                    text,
                    None,
                )
                .await;
            None
        }
    }
}

/// Закрыто по событию: опрос статуса продолжается до окончательного статуса — подтверждение; у DEX он же снимает
/// ожидание поступления в кошелёк (wallet/inflows.rs). Опрос не дольше transfer.max_time_s от начала перевода (не
/// задана — только окно transfer.credit_mismatch_s; нет и её — опроса нет). Не «зачислен» за transfer.credit_mismatch_s
/// или «отклонён» — журнал (один раз). Ошибок нет.
pub async fn confirm(core: Core, o: Owned, gw: Arc<dyn Venue>, ticket: Option<Ticket>) {
    drop(ticket);
    let max = limit(&core, &o).await;
    let at = At {
        venue: Some(&o.to.0),
        coin: Some(&o.asset.ticker),
    };
    let total = core
        .inner
        .deps
        .settings
        .integer("transfer.max_time_s", at)
        .ok();
    let begun = tokio::time::Instant::now();
    let spent = (chrono::Utc::now() - o.started)
        .to_std()
        .unwrap_or_default();
    let Some(end) = total
        .map(|t| Duration::from_secs(t).saturating_sub(spent))
        .or(max)
        .map(|d| begun + d)
    else {
        return;
    };
    let quiet = Notify::new();
    let mut pacer = Pacer::new("venue.status_poll_ms", Unit::Ms, Some(o.to.0.clone()));
    let mut told = false;
    while tokio::time::Instant::now() < end {
        let status = match gw.deposit_status(&o.asset, &o.tx).await {
            Ok(t) if t.status == TransferStatus::Credited => return,
            Ok(t) if t.status == TransferStatus::Rejected => {
                let text = format!(
                    "{}: зачислено по событию, а статус депозита {} — «отклонён»",
                    o.deal, o.tx
                );
                report(&core, &o.to, text).await;
                return;
            }
            Ok(t) => format!("{:?}", t.status),
            Err(e) => e.message,
        };
        if let Some(max) = max.filter(|m| !told && begun.elapsed() >= *m) {
            told = true;
            let text = format!(
                "{}: зачислено по событию, а статус депозита {} — не «зачислен» за transfer.credit_mismatch_s ({} с): {status}",
                o.deal,
                o.tx,
                max.as_secs()
            );
            report(&core, &o.to, text).await;
        }
        tokio::select! {
            _ = pacer.pause(&core.inner.deps, &quiet) => {}
            _ = tokio::time::sleep_until(end) => {}
        }
    }
}

/// Зачислено по статусу: ожидание держится ещё transfer.credit_mismatch_s — позднее событие этого перевода
/// сопоставится с ним, а не станет «внешним» (не задана — ожидание снимается сразу). Площадка шлёт события, а своего
/// события за окно нет — журнал. Ошибок нет.
pub async fn no_event(core: Core, o: Owned, ticket: Ticket) {
    if ticket.has_event() {
        return;
    }
    let Some(max) = limit(&core, &o).await else {
        return;
    };
    let end = tokio::time::Instant::now() + max;
    while !ticket.has_event() {
        if tokio::time::timeout_at(end, ticket.wake.notified())
            .await
            .is_err()
        {
            break;
        }
    }
    if ticket.has_event() || !ticket.venue_seen(&o.to) {
        return;
    }
    let text = format!(
        "{}: статус депозита {} «зачислен», а события зачисления {} нет за transfer.credit_mismatch_s ({} с)",
        o.deal,
        o.tx,
        o.asset.ticker,
        max.as_secs()
    );
    report(&core, &o.to, text).await;
}

/// Расхождение: журнал core.credit_mismatch и уведомление exec.credit_mismatch. Ошибок нет.
pub async fn report(core: &Core, to: &VenueId, text: String) {
    let deps = &core.inner.deps;
    deps.log(
        Level::Warn,
        "core.credit_mismatch",
        Some(&to.0),
        &text,
        None,
    )
    .await;
    deps.notify("exec.credit_mismatch", Some(&to.0), &text)
        .await;
}
