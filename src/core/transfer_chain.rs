//! Файл: core/transfer_chain.rs
//! Что делает: решение 151 — перевод сделки на кошелёк DEX зачислен по приходу монеты в блокчейне (credit_chain.rs),
//! не дожидаясь, пока биржа-отправитель отдаст в API хеш и статус вывода:
//! - зачисление сразу: время в пути, снятие карантина, журнал и уведомление deal.transfer_credited (transfer_credit.rs
//!   done), перевод на торговый счёт, подтверждение статусом депозита по хешу пришедшей транзакции в фоне
//!   (credit_mismatch.rs confirm — у DEX он же снимает ожидание поступления, wallet/inflows.rs), проверка недостачи
//!   (credit_shortfall.rs); хеш пришедшей транзакции — в перевод сделки (tx_hash) и в хронологию;
//! - хеш вывода от биржи — фоном тем же опросом withdrawal_status (venue.status_poll_ms отправителя, не дольше
//!   transfer.max_time_s от начала перевода), сделку не держит: совпал — журнал; другой хеш или вывод «отклонён» —
//!   Level::Error и уведомление exec.chain_hash_mismatch; не получен за срок — Warn и то же уведомление. Сделку не трогаем.
//!
//! Кто вызывает: core/transfers.rs (send_trade).
//! От чего зависит: transfer_credit.rs, credit_mismatch.rs, credit_shortfall.rs, credit_watch.rs (norm), credit_chain.rs
//! (строка хронологии), wait.rs, deps.rs (шлюз, журнал, уведомления).
//! Настройки: venue.status_poll_ms (отправитель), transfer.max_time_s (получатель и монета).

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tokio::sync::Notify;

use crate::core::types::{Asset, BalanceChange, Transfer, TransferStatus, VenueId};
use crate::gateway::contract::Venue;
use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::credit_watch::{Ticket, norm};
use super::engine::Core;
use super::transfer_credit::Leg;
use super::wait::{Pacer, Unit};

/// Что известно о переводе к моменту зачисления по блокчейну.
pub struct Chain<'a> {
    pub deal: &'a str,
    pub from: &'a VenueId,
    pub to: &'a VenueId,
    pub asset: &'a Asset,
    /// Сеть отправителя (для проверки недостачи) и получателя (для зачисления).
    pub net_from: &'a str,
    pub net_to: String,
    /// Сумма вывода.
    pub qty: Decimal,
    /// Момент запроса вывода.
    pub started: DateTime<Utc>,
    /// Ответ биржи на запрос вывода (id вывода, комиссия).
    pub created: Transfer,
    pub gw_from: Arc<dyn Venue>,
    pub gw_to: Arc<dyn Venue>,
}

/// Зачисляет перевод по пришедшему поступлению. Принимает: ядро, перевод, поступление (credit_chain.rs), ожидание.
/// Возвращает: (вывод — ответ биржи с хешем пришедшей транзакции, зачисление, строка хронологии). Ошибок нет.
pub async fn finish(
    core: &Core,
    c: Chain<'_>,
    hit: BalanceChange,
    ticket: Option<Ticket>,
) -> (Transfer, Transfer, String) {
    let hash = hit.tx_hash.clone().unwrap_or_default();
    let leg = Leg {
        deal: c.deal,
        from: c.from,
        to: c.to,
        asset: c.asset,
        network: c.net_to.clone(),
        tx: hash.clone(),
        started: c.started,
    };
    let credited = Transfer {
        venue: c.to.clone(),
        asset: c.asset.clone(),
        network: c.net_to.clone(),
        amount: hit.delta,
        status: TransferStatus::Credited,
        venue_ref: String::new(),
        tx_hash: Some(hash.clone()),
        fee: None,
        updated_at: hit.at,
    };
    let by = format!("по блокчейну, не дожидаясь API {}", c.from.0);
    super::transfer_credit::done(core, &leg, &credited, hit.at, &by).await;
    super::transfer_credit::to_trading(core, &leg, &c.gw_to, &credited).await;
    tokio::spawn(super::credit_mismatch::confirm(
        core.clone(),
        super::credit_mismatch::owned(&leg),
        c.gw_to.clone(),
        ticket,
    ));
    let mut sent = c.created.clone();
    sent.tx_hash = Some(hash.clone());
    sent.status = TransferStatus::Sent;
    tokio::spawn(sender_hash(
        core.clone(),
        Check {
            deal: c.deal.to_string(),
            from: c.from.clone(),
            to: c.to.clone(),
            ticker: c.asset.ticker.clone(),
            coin: crate::settings::scope::coin_id(&c.asset.ticker, c.asset.bsc_contract.as_ref()),
            venue_ref: c.created.venue_ref.clone(),
            hash: hash.clone(),
            started: c.started,
            gw: c.gw_from.clone(),
        },
    ));
    let (d, f, t, a) = (c.deal, c.from, c.to, c.asset);
    super::credit_shortfall::check(core, d, f, t, a, c.net_from, c.qty, credited.amount).await;
    let note = super::credit_chain::note(c.from, credited.amount, &c.asset.ticker, &hash);
    (sent, credited, note)
}

/// Данные фоновой сверки хеша вывода.
struct Check {
    deal: String,
    from: VenueId,
    to: VenueId,
    ticker: String,
    /// Решение 211: id области «монета» настроек (адрес; без адреса — тикер).
    coin: String,
    venue_ref: String,
    /// Хеш пришедшей транзакции.
    hash: String,
    started: DateTime<Utc>,
    gw: Arc<dyn Venue>,
}

/// Фоном: хеш вывода от биржи и сверка с пришедшей транзакцией (опрос withdrawal_status, не дольше
/// transfer.max_time_s от начала перевода; не задана — сверки нет, журнал). Ошибок нет: итог — в журнал.
async fn sender_hash(core: Core, c: Check) {
    let deps = &core.inner.deps;
    let at = At {
        venue: Some(&c.to.0),
        coin: Some(&c.coin),
    };
    let head = format!(
        "{}: {} зачислено по блокчейну (транзакция {})",
        c.deal,
        c.ticker,
        crate::notify::human::addr(&c.hash)
    );
    let total = match deps.settings.integer("transfer.max_time_s", at) {
        Ok(s) => Duration::from_secs(s),
        Err(e) => {
            let text = format!(
                "{head}; хеш вывода от {} не сверяется — {}",
                c.from.0, e.message
            );
            deps.log(
                Level::Warn,
                "core.transfer_chain",
                Some(&c.from.0),
                text,
                None,
            )
            .await;
            return;
        }
    };
    let spent = (Utc::now() - c.started).to_std().unwrap_or_default();
    let end = tokio::time::Instant::now() + total.saturating_sub(spent);
    let quiet = Notify::new();
    let mut pacer = Pacer::new("venue.status_poll_ms", Unit::Ms, Some(c.from.0.clone()));
    while tokio::time::Instant::now() < end {
        match c.gw.withdrawal_status(&c.venue_ref).await {
            Ok(t) if t.tx_hash.is_some() && t.status != TransferStatus::Pending => {
                let tx = t.tx_hash.unwrap_or_default();
                if norm(&tx) == norm(&c.hash) {
                    let text = format!("{head}; хеш вывода от {} совпал", c.from.0);
                    deps.log(
                        Level::Info,
                        "core.transfer_chain",
                        Some(&c.from.0),
                        text,
                        None,
                    )
                    .await;
                } else {
                    let text = format!(
                        "{head}, а {} отдала хеш вывода {tx} — другая транзакция. Сделку не трогаем, проверьте вручную",
                        c.from.0
                    );
                    report(&core, &c.from, Level::Error, text).await;
                }
                return;
            }
            Err(e) if e.code == ErrorCode::WithdrawRejected => {
                let text = format!(
                    "{head}, а {} сообщает: вывод отклонён ({}). Сделку не трогаем, проверьте вручную",
                    c.from.0, e.message
                );
                report(&core, &c.from, Level::Error, text).await;
                return;
            }
            _ => {}
        }
        tokio::select! {
            _ = pacer.pause(deps, &quiet) => {}
            _ = tokio::time::sleep_until(end) => {}
        }
    }
    let text = format!(
        "{head}; хеш вывода от {} не получен за отведённое время — проверьте вручную",
        crate::notify::human::venue(&c.from.0)
    );
    report(&core, &c.from, Level::Warn, text).await;
}

/// Расхождение: журнал core.transfer_chain и уведомление exec.chain_hash_mismatch. Ошибок нет.
async fn report(core: &Core, from: &VenueId, level: Level, text: String) {
    let deps = &core.inner.deps;
    deps.log(level, "core.transfer_chain", Some(&from.0), &text, None)
        .await;
    let tg = crate::notify::human::from_core("exec.chain_hash_mismatch", Some(&from.0), &text);
    deps.notify("exec.chain_hash_mismatch", Some(&from.0), &tg)
        .await;
}
