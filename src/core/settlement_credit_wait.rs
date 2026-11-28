//! Файл: core/settlement_credit_wait.rs
//! Что делает: денежный круг Base (уточнение Тиграна 28.11.2026) — ждёт зачисления на получателе так же, как обычный
//! перевод BSC (transfer_credit::wait_with_units, ветка с записью депозита): опрос записи депозита
//! deposit_status_for(DepositClaim) с частотой venue.status_poll_ms, событие баланса будит опрос раньше (а если в нём
//! хеш вывода — передаётся адаптеру как подтверждённая сумма), долгий перевод — карантин (transfer_watch::slow), по
//! зачислению — transfer_credit::done (время в пути, снятие карантина, журнал, «Перевод зачислен»).
//! Отличие от обычного перевода — только одно: зачисление засчитывается ТОЛЬКО по записи депозита (событие баланса
//! само по себе зачисление не закрывает), потому что круг дальше сверяет хеш, сумму и сеть записи
//! (settlement_transfer.rs verified) — проверки круга не ослабляются. Перевод на торговый счёт — отдельным шагом
//! круга (settlement_to_trading.rs), с отметкой для повтора после перезапуска.
//! Кто вызывает: core/transfers.rs (go, ветка круга), core/settlement_transfer.rs (восстановление вывода).
//! От чего зависит: transfer_credit.rs (Leg, done, by_event), credit_watch.rs (Ticket, norm), transfer_watch.rs,
//! wait.rs (Pacer), шлюз (deposit_status_for).
//! Настройки: venue.status_poll_ms (по площадке получателя), transfer.max_time_s (карантин) — как у обычного перевода.

use std::sync::Arc;

use chrono::Utc;
use tokio::sync::Notify;

use crate::core::types::{Asset, Transfer, TransferStatus};
use crate::gateway::contract::{DepositClaim, Venue};
use crate::storage::journal::Level;

use super::credit_watch::Ticket;
use super::engine::Core;
use super::transfer_credit::Leg;
use super::wait::{Pacer, Unit};

/// Что делает: ждёт, пока запись депозита на получателе станет «зачислено» (шапка). Принимает: ядро, перевод, шлюз
/// получателя, монету получателя, заявку отправителя, ожидание события (None — только опрос). Возвращает: запись
/// депозита со статусом Credited. Ошибок нет — ждёт до зачисления, ошибки опроса — в журнал (при смене текста).
pub(super) async fn wait(
    core: &Core,
    leg: &Leg<'_>,
    gw: &Arc<dyn Venue>,
    on: &Asset,
    claim: &DepositClaim,
    ticket: Option<&Ticket>,
) -> Transfer {
    let deps = &core.inner.deps;
    let quiet = Notify::new();
    let mut pacer = Pacer::new("venue.status_poll_ms", Unit::Ms, Some(leg.to.0.clone()));
    let mut last = String::new();
    loop {
        // Как transfer_credit (ветка записи депозита): сумма события — только если в нём хеш этого вывода.
        let delta = super::transfer_credit::by_event(ticket)
            .filter(|ev| {
                ev.tx_hash.as_ref().is_some_and(|h| {
                    super::credit_watch::norm(h) == super::credit_watch::norm(&leg.tx)
                })
            })
            .map(|ev| ev.delta);
        let mut claim = claim.clone();
        if claim.expected.is_none() {
            claim.expected = ticket.and_then(|t| t.amount_observed(&leg.tx));
        }
        match gw.deposit_status_for(on, &leg.tx, &claim, delta).await {
            Ok(t) if t.status == TransferStatus::Credited => {
                super::transfer_credit::done(core, leg, &t, Utc::now(), "статус депозита").await;
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
        let kick = ticket.map(|t| t.wake.as_ref()).unwrap_or(&quiet);
        pacer.pause(deps, kick).await;
    }
}
