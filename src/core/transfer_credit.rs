//! Файл: core/transfer_credit.rs
//! Что делает: шаг «ждём зачисления на получателе» перевода (transfers.rs) — событие или опрос, кто первый (решение 76):
//! - событие баланса подошло к ожиданию и закрывает его (credit_watch.rs: хеш события совпал с хешем вывода от
//!   отправителя или хеша в событии нет; transfer.credit_by_event) — зачислено сразу, время в пути — от момента события;
//!   опрос статуса продолжается в фоне как подтверждение;
//! - опрос deposit_status с частотой venue.status_poll_ms (событие будит его раньше) — зачислено по статусу.
//!
//! Расхождение события и статуса дольше transfer.credit_mismatch_s — журнал (credit_mismatch.rs).
//!
//! Кто вызывает: core/transfers.rs.
//! От чего зависит: credit_watch.rs, credit_mismatch.rs, transit.rs, transfer_watch.rs, wait.rs, deps.rs (шлюз).
//! После зачисления — перевод на торговый счёт площадки (решение 102, Venue::deposit_to_trading; у BingX — фондовый → спот).
//! Настройки: venue.status_poll_ms (по площадке получателя); venue.deposit_to_spot — у адаптера BingX.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tokio::sync::Notify;

use crate::core::types::{Asset, Transfer, TransferStatus, VenueId};
use crate::gateway::contract::Venue;
use crate::gateway::errors::ErrorCode;
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
    loop {
        if let Some(hit) = by_event(ticket.as_ref()) {
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
            to_trading(core, leg, &gw, &t).await;
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
                to_trading(core, leg, &gw, &t).await;
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

/// Решение 102: зачисленное — на счёт, где площадка торгует (у BingX — с фондового на спот, venue.deposit_to_spot).
/// Сбой — по правилам шлюза (повтор или стоп, core/rules_apply.rs; «итог неизвестен» не повторяется), журнал и
/// уведомление transfer.to_trading_failed: монеты или USDT остаются на фондовом счёте — продажа сделки там не пройдёт и
/// уйдёт в «Ручное управление» (решение 100). Принимает: перевод, шлюз получателя, зачисление. Ошибок нет.
async fn to_trading(core: &Core, leg: &Leg<'_>, gw: &Arc<dyn Venue>, t: &Transfer) {
    let deps = &core.inner.deps;
    let mut attempt = 0;
    loop {
        attempt += 1;
        match gw.deposit_to_trading(leg.asset, t.amount).await {
            Ok(None) => return,
            Ok(Some(text)) => {
                let text = format!("{}: {text}", leg.deal);
                deps.log(
                    Level::Info,
                    "core.transfer_to_trading",
                    Some(&leg.to.0),
                    &text,
                    None,
                )
                .await;
                return;
            }
            Err(e) => {
                let next = super::rules_apply::on_error(
                    core,
                    "Перевод зачисленного на торговый счёт",
                    leg.to,
                    Some(leg.deal),
                    &e,
                    attempt,
                )
                .await;
                match next {
                    super::rules_apply::Next::Retry { pause_ms }
                        if e.code != ErrorCode::OutcomeUnknown =>
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(pause_ms)).await
                    }
                    _ => {
                        let text = format!(
                            "{} {} {}: зачислено на {}, но не переведено на торговый счёт: {} — переведите вручную",
                            leg.deal, t.amount, leg.asset.ticker, leg.to.0, e.message
                        );
                        deps.log(
                            Level::Error,
                            "core.transfer_to_trading",
                            Some(&leg.to.0),
                            &text,
                            None,
                        )
                        .await;
                        deps.notify("transfer.to_trading_failed", Some(&leg.to.0), &text)
                            .await;
                        return;
                    }
                }
            }
        }
    }
}

/// Событие, закрывающее зачисление (credit_watch.rs Ticket::closing: своё — хеш совпал с хешем отправителя или хеша в
/// событии нет — и закрытие включено). None — ждём опрос (или ожидания нет). Ошибок нет.
pub fn by_event(ticket: Option<&Ticket>) -> Option<crate::core::types::BalanceChange> {
    ticket.and_then(Ticket::closing)
}
