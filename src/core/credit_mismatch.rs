//! Файл: core/credit_mismatch.rs
//! Что делает: сверка зачисления по событию и по статусу после того, как перевод уже зачислен (решение 76):
//! - зачислено по событию — опрос deposit_status продолжается до окончательного статуса (подтверждение; у DEX он же
//!   снимает ожидание поступления в кошелёк, wallet/inflows.rs); не «зачислен» за transfer.credit_mismatch_s или
//!   «отклонён» — журнал;
//! - зачислено по статусу — событие должно прийти за transfer.credit_mismatch_s (только у площадки, приславшей хоть
//!   одно событие баланса с запуска); не пришло — журнал.
//!
//! Записи — вид core.credit_mismatch, уведомление exec.credit_mismatch.
//!
//! Кто вызывает: core/transfer_credit.rs.
//! От чего зависит: credit_watch.rs (Ticket), wait.rs, deps.rs (шлюз, журнал), settings.
//! Настройки: transfer.credit_mismatch_s, venue.status_poll_ms (по площадке получателя).

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
}

/// Данные шага для фоновой сверки. Ошибок нет.
pub fn owned(leg: &Leg<'_>) -> Owned {
    Owned {
        deal: leg.deal.to_string(),
        to: leg.to.clone(),
        asset: leg.asset.clone(),
        tx: leg.tx.clone(),
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
/// ожидание поступления в кошелёк (wallet/inflows.rs), поэтому идёт и без transfer.credit_mismatch_s. Статус
/// «зачислен» не пришёл за transfer.credit_mismatch_s — журнал (один раз), опрос продолжается. Ошибок нет.
pub async fn confirm(core: Core, o: Owned, gw: Arc<dyn Venue>, ticket: Option<Ticket>) {
    drop(ticket);
    let max = limit(&core, &o).await;
    let quiet = Notify::new();
    let mut pacer = Pacer::new("venue.status_poll_ms", Unit::Ms, Some(o.to.0.clone()));
    let started = tokio::time::Instant::now();
    let mut told = false;
    loop {
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
        if let Some(max) = max.filter(|m| !told && started.elapsed() >= *m) {
            told = true;
            let text = format!(
                "{}: зачислено по событию, а статус депозита {} — не «зачислен» за transfer.credit_mismatch_s ({} с): {status}; опрос продолжается",
                o.deal,
                o.tx,
                max.as_secs()
            );
            report(&core, &o.to, text).await;
        }
        pacer.pause(&core.inner.deps, &quiet).await;
    }
}

/// Зачислено по статусу: событие должно прийти за transfer.credit_mismatch_s, если площадка шлёт события. Ошибок нет.
pub async fn no_event(core: Core, o: Owned, ticket: Ticket) {
    if ticket.hit().is_some() {
        return;
    }
    let Some(max) = limit(&core, &o).await else {
        return;
    };
    let _ = tokio::time::timeout(max, ticket.wake.notified()).await;
    if ticket.hit().is_some() || !ticket.venue_seen(&o.to) {
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
