//! Файл: core/transfers.rs
//! Что делает: перевод монеты или USDT между площадками по сети BSC (шаг 5 ТЗ): адрес депозита на
//! получателе → вывод с отправителя → хеш транзакции → зачисление. Статусы — через шлюз с частотой
//! venue.status_poll_ms; зачисление — по событию баланса или опросу, кто первый (решение 76: до вывода регистрируется
//! ожидаемое зачисление — credit_expect.rs, credit_watch.rs; шаг зачисления — transfer_credit.rs). Перевод дольше transfer.max_time_s —
//! уведомление и карантин монеты, ждём дальше (монеты в пути). Фактическое время в пути пишется в историю
//! (для T в расчёте, dirctx.rs).
//! Кто вызывает: executor.rs (перевод сделки), stuck.rs (перегон), usdt_return.rs (USDT).
//! От чего зависит: engine.rs, wait.rs, rules_apply.rs, risk.rs, registry (сети BSC площадок), credit_expect.rs,
//! credit_watch.rs, transfer_credit.rs.
//! Настройки: venue.status_poll_ms (по площадке), transfer.max_time_s (по площадке получателя и монете).

use std::time::Duration;

use chrono::Utc;
use rust_decimal::Decimal;
use tokio::sync::Notify;

use crate::core::types::{Asset, Transfer, TransferStatus, VenueId, WithdrawRequest};
use crate::gateway::errors::ErrorCode;
use crate::storage::journal::Level;

use super::engine::{Core, rd};
use super::op_error::OpError;
use super::rules_apply::{self, Next};
use super::wait::{Pacer, Unit};

/// Название сети BSC площадки для монеты (у биржи — из её справочника; у DEX — сеть любой биржи монеты).
/// Ошибка: ни у одной биржи монеты нет сети BSC.
pub fn network(core: &Core, asset: &Asset, venue: &VenueId) -> Result<String, String> {
    let coins = rd(&core.inner.coins);
    let entry = coins
        .iter()
        .find(|c| c.asset.bsc_contract == asset.bsc_contract && asset.bsc_contract.is_some());
    let any = rd(&core.inner.networks)
        .get(venue)
        .and_then(|l| {
            l.iter()
                .find(|i| i.asset.ticker == asset.ticker && i.asset.bsc_contract.is_some())
        })
        .map(|i| i.network.clone());
    let own = entry.and_then(|e| {
        e.presence
            .iter()
            .find(|p| &p.venue == venue)
            .and_then(|p| p.network.clone())
    });
    let other = entry.and_then(|e| e.presence.iter().find_map(|p| p.network.clone()));
    own.or(any)
        .or(other)
        .or_else(|| {
            // USDT и другие монеты вне справочника: сеть BSC биржи — у любой её монеты с адресом контракта.
            rd(&core.inner.networks)
                .values()
                .flatten()
                .find(|i| i.asset.bsc_contract.is_some())
                .map(|i| i.network.clone())
        })
        .ok_or_else(|| format!("{}: нет сети BSC для {}", venue.0, asset.ticker))
}

/// Переводит qty монеты с from на to. Возвращает: (вывод с хешем и комиссией, зачисление — сколько дошло).
/// Ошибки: текст причины (вывод отклонён, правило шлюза остановило) — уже в журнале и уведомлениях.
pub async fn send(
    core: &Core,
    deal: &str,
    from: &VenueId,
    to: &VenueId,
    asset: &Asset,
    qty: Decimal,
) -> Result<(Transfer, Transfer), OpError> {
    let deps = &core.inner.deps;
    let gw_from = deps.gw.get(from).await.map_err(|e| e.message)?;
    let gw_to = deps.gw.get(to).await.map_err(|e| e.message)?;
    let net_from = network(core, asset, from)?;
    let net_to = network(core, asset, to)?;
    let address = gw_to
        .deposit_address(asset, &net_to)
        .await
        .map_err(|e| format!("{}: адрес депозита: {}", to.0, e.message))?;
    // Ожидаемое зачисление — до вывода, чтобы событие не пришло раньше (решение 76). Приход не посчитан — только опрос.
    let ticket = match super::credit_expect::arrival(core, from, asset, &net_from, qty).await {
        Ok(a) => Some(super::credit_watch::register(
            core, deal, from, to, asset, a,
        )),
        Err(e) => {
            let text = format!(
                "{deal}: зачисление {} только по опросу статуса — {e}",
                asset.ticker
            );
            deps.log(Level::Info, "core.credit_event", Some(&to.0), text, None)
                .await;
            None
        }
    };
    let req = WithdrawRequest {
        venue: from.clone(),
        asset: asset.clone(),
        network: net_from,
        address,
        amount: qty,
        deal_id: deal.to_string(),
    };
    let started = Utc::now();
    let mut attempt = 0;
    let created = loop {
        attempt += 1;
        match gw_from.withdraw(req.clone()).await {
            Ok(t) => break t,
            Err(e) => match rules_apply::on_error(core, "Вывод", from, Some(deal), &e, attempt)
                .await
            {
                Next::Retry { pause_ms } => {
                    tokio::time::sleep(Duration::from_millis(pause_ms)).await
                }
                Next::Stop => {
                    // Вывода не будет — получатель снимает ожидание поступления (DEX: wallet/inflows.rs). Исход
                    // неизвестен — перевод мог уйти: ожидание остаётся (до ручной проверки или срока).
                    if e.code != ErrorCode::OutcomeUnknown {
                        let _ = gw_to.deposit_abandoned(asset).await;
                    }
                    return Err(OpError::from(&e));
                }
            },
        }
    };
    let quiet = Notify::new();
    // 1. Ждём хеш транзакции вывода.
    let mut pacer = Pacer::new("venue.status_poll_ms", Unit::Ms, Some(from.0.clone()));
    let mut last = String::new();
    let sent = loop {
        // Событие зачисления с хешем пришло раньше хеша от отправителя — хеш из события (решение 76).
        if let Some((hit, true)) = ticket.as_ref().and_then(|t| t.hit())
            && let Some(h) = hit.tx_hash
        {
            break Transfer {
                tx_hash: Some(h),
                status: TransferStatus::Sent,
                ..created.clone()
            };
        }
        match gw_from.withdrawal_status(&created.venue_ref).await {
            Ok(t) if t.tx_hash.is_some() && t.status != TransferStatus::Pending => break t,
            Ok(_) => {}
            Err(e) if e.code == ErrorCode::WithdrawRejected => {
                rules_apply::on_error(core, "Вывод", from, Some(deal), &e, 1).await;
                let _ = gw_to.deposit_abandoned(asset).await;
                return Err(OpError::from(&e));
            }
            Err(e) if e.message != last => {
                deps.log(
                    Level::Warn,
                    "core.transfer",
                    Some(&from.0),
                    format!("Статус вывода {}: {}", created.venue_ref, e.message),
                    None,
                )
                .await;
                last = e.message;
            }
            Err(_) => {}
        }
        super::transfer_watch::slow(core, deal, to, asset, started).await;
        let kick = ticket.as_ref().map(|t| t.wake.as_ref()).unwrap_or(&quiet);
        pacer.pause(deps, kick).await;
    };
    let tx = sent.tx_hash.clone().unwrap_or_default();
    let text = format!(
        "{} {qty} {}: {} → {}, хеш {tx}",
        deal, asset.ticker, from.0, to.0
    );
    deps.log(
        Level::Info,
        "core.transfer_sent",
        Some(&from.0),
        &text,
        None,
    )
    .await;
    deps.notify("deal.transfer_sent", Some(&from.0), &text)
        .await;
    // 2. Ждём зачисления на получателе: событие или опрос (transfer_credit.rs).
    let leg = super::transfer_credit::Leg {
        deal,
        from,
        to,
        asset,
        network: net_to,
        tx,
        started,
    };
    let credited = super::transfer_credit::wait(core, &leg, gw_to, ticket).await;
    Ok((sent, credited))
}
