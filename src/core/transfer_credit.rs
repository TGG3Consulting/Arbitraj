//! Файл: core/transfer_credit.rs
//! Что делает: шаг «ждём зачисления на получателе» перевода (transfers.rs) — событие или опрос, кто первый (решение 76):
//! - событие баланса подошло к ожиданию и закрывает его (credit_watch.rs, transfer.credit_by_event) — зачислено сразу,
//!   время в пути — от момента события; опрос статуса продолжается в фоне как подтверждение;
//! - опрос deposit_status с частотой venue.status_poll_ms (событие будит его раньше) — зачислено по статусу.
//!
//! Расхождение события и статуса дольше transfer.credit_mismatch_s — журнал (credit_mismatch.rs).
//!
//! Кто вызывает: core/transfers.rs.
//! От чего зависит: credit_watch.rs, credit_mismatch.rs, transit.rs, transfer_watch.rs, wait.rs, deps.rs (шлюз).
//! Настройки: venue.status_poll_ms (по площадке получателя).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tokio::sync::Notify;

use crate::core::types::{Asset, Transfer, TransferStatus, VenueId};
use crate::gateway::contract::Venue;
use crate::storage::journal::Level;

use super::credit_watch::Ticket;
use super::engine::Core;
use super::wait::{Pacer, Unit};

/// Что знает шаг зачисления.
pub struct Leg<'a> {
    pub deal: &'a str,
    pub from: &'a VenueId,
    pub to: &'a VenueId,
    pub asset: &'a Asset,
    pub network: String,
    /// Хеш транзакции перевода.
    pub tx: String,
    pub started: DateTime<Utc>,
}

/// Ждёт зачисления: событие или опрос. Возвращает: зачисление (сколько дошло). Ошибок нет — ждёт, пока не зачислено
/// (долгий перевод — карантин, transfer_watch.rs).
pub async fn wait(
    core: &Core,
    leg: &Leg<'_>,
    gw: Arc<dyn Venue>,
    ticket: Option<Ticket>,
) -> Transfer {
    let deps = &core.inner.deps;
    let quiet = Notify::new();
    let mut pacer = Pacer::new("venue.status_poll_ms", Unit::Ms, Some(leg.to.0.clone()));
    let mut last = String::new();
    let mut told_hash = false;
    loop {
        let hit = ticket.as_ref().and_then(|t| t.hit());
        // Событие с другим хешем, чем у вывода, — не этот перевод: не закрываем, решает опрос (один раз в журнал).
        let other = hit
            .as_ref()
            .and_then(|(h, _)| h.tx_hash.as_ref())
            .filter(|h| !h.eq_ignore_ascii_case(&leg.tx))
            .cloned();
        if let (Some(h), false) = (other.as_ref(), told_hash) {
            told_hash = true;
            let text = format!(
                "{}: событие зачисления {} с хешем {h}, а у вывода хеш {} — по событию не закрываем, ждём опрос",
                leg.deal, leg.asset.ticker, leg.tx
            );
            super::credit_mismatch::report(core, leg.to, text).await;
        }
        if let (Some((hit, true)), None) = (hit, &other) {
            let t = Transfer {
                venue: leg.to.clone(),
                asset: leg.asset.clone(),
                network: leg.network.clone(),
                amount: hit.delta,
                status: TransferStatus::Credited,
                venue_ref: String::new(),
                tx_hash: Some(leg.tx.clone()),
                fee: None,
                updated_at: hit.at,
            };
            done(core, leg, &t, hit.at, &hit.source).await;
            tokio::spawn(super::credit_mismatch::confirm(
                core.clone(),
                super::credit_mismatch::owned(leg),
                gw,
                ticket,
            ));
            return t;
        }
        match gw.deposit_status(leg.asset, &leg.tx).await {
            Ok(t) if t.status == TransferStatus::Credited => {
                done(core, leg, &t, Utc::now(), "статус депозита").await;
                if let Some(tk) = ticket {
                    tokio::spawn(super::credit_mismatch::no_event(
                        core.clone(),
                        super::credit_mismatch::owned(leg),
                        tk,
                    ));
                }
                return t;
            }
            Ok(_) => {}
            Err(e) if e.message != last => {
                let text = format!("Депозит {}: {}", leg.tx, e.message);
                deps.log(Level::Info, "core.transfer", Some(&leg.to.0), text, None)
                    .await;
                last = e.message;
            }
            Err(_) => {}
        }
        super::transfer_watch::slow(core, leg.deal, leg.to, leg.asset, leg.started).await;
        let kick = ticket.as_ref().map(|t| t.wake.as_ref()).unwrap_or(&quiet);
        pacer.pause(deps, kick).await;
    }
}

/// Зачислено: время в пути, снятие карантина, журнал и уведомление. Принимает: перевод, зачисление, момент, откуда
/// известно. Ошибок нет.
async fn done(core: &Core, leg: &Leg<'_>, t: &Transfer, at: DateTime<Utc>, by: &str) {
    let deps = &core.inner.deps;
    let ms = (at - leg.started).num_milliseconds().max(0);
    let secs = Decimal::from(ms) / Decimal::ONE_THOUSAND;
    super::transit::record(core, leg.asset, leg.from, leg.to, secs).await;
    super::transfer_watch::release(core, leg.asset, leg.to).await;
    let text = format!(
        "{} {} {}: зачислено на {} за {secs} с ({by})",
        leg.deal, t.amount, leg.asset.ticker, leg.to.0
    );
    deps.log(
        Level::Info,
        "core.transfer_credited",
        Some(&leg.to.0),
        &text,
        None,
    )
    .await;
    deps.notify("deal.transfer_credited", Some(&leg.to.0), &text)
        .await;
}
