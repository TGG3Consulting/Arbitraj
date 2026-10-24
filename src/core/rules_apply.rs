//! Файл: core/rules_apply.rs
//! Что делает: применение правил шлюза к ошибке операции ядра (CLAUDE.md, «Архитектура»): правило
//! площадки сильнее общего (gateway/rules.rs, pick_rule); правила вводит Тигран в интерфейсе, в коде их нет.
//! Стартовое правило Тиграна «ордер не исполнился — отменяем, больше не пробуем» — это CancelNoRetry.
//! Правила нет — операция останавливается и уведомление уходит (gateway/README.md).
//! Каждая ошибка — в журнал и в уведомление своего события; ошибки подряд считает risk.rs.
//! Кто вызывает: executor.rs, transfers.rs, stuck.rs, usdt_return.rs.
//! От чего зависит: gateway (errors, rules), storage/rules.rs, risk.rs, deps.rs.
//! Настройки: правила шлюза (SQLite), risk.max_errors_in_row (через risk.rs).

use serde_json::json;

use crate::core::types::VenueId;
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::gateway::rules::{RuleAction, pick_rule};
use crate::storage::journal::Level;

use super::engine::Core;
use super::risk::{self, Mode};

/// Что делать дальше.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Повторить операцию после паузы (мс).
    Retry { pause_ms: u64 },
    /// Остановить операцию.
    Stop,
}

/// Событие уведомлений для кода ошибки (notify/catalog.rs). None — отдельного события нет.
fn event_of(code: ErrorCode) -> Option<&'static str> {
    Some(match code {
        ErrorCode::OrderNotFilled | ErrorCode::PartialFill => "exec.order_not_filled",
        ErrorCode::SwapReverted => "exec.swap_reverted",
        ErrorCode::WithdrawRejected => "exec.withdraw_rejected",
        ErrorCode::DepositNotCredited => "exec.deposit_not_credited",
        ErrorCode::RateLimited => "venue.rate_limited",
        ErrorCode::ChannelRejected => "send.channel_rejected",
        ErrorCode::TxNotIncluded => "send.tx_not_included",
        _ => return None,
    })
}

/// Ошибка операции. Принимает: ядро, что делали, площадку, сделку, ошибку, номер попытки (с 1).
/// Возвращает: Next по правилу шлюза. Ошибок нет: сбой чтения правил — операция останавливается.
pub async fn on_error(
    core: &Core,
    op: &str,
    venue: &VenueId,
    deal: Option<&str>,
    err: &GatewayError,
    attempt: u32,
) -> Next {
    let deps = &core.inner.deps;
    let text = format!("{op} на {}: {} ({:?})", venue.0, err.message, err.code);
    deps.log(
        Level::Error,
        "core.op_error",
        Some(&venue.0),
        &text,
        Some(json!({ "deal": deal, "error": err })),
    )
    .await;
    // Исход неизвестен (транзакция могла исполниться): повтор — риск двойного вывода или обмена. Не повторяем при
    // любых правилах шлюза, площадка — на паузу до ручной проверки. Уведомление и пауза — один раз на смену
    // состояния: остановленная очередь кошелька отвечает так на каждую попытку.
    if err.code == ErrorCode::OutcomeUnknown {
        let why = format!("{op}: исход транзакции неизвестен — нужна ручная проверка");
        let fresh = {
            let mut r = super::engine::lk(&core.inner.risk);
            if r.venue_paused.contains_key(venue) {
                false
            } else {
                r.venue_paused.insert(venue.clone(), why.clone());
                true
            }
        };
        if fresh {
            risk::publish(core);
            deps.notify("send.outcome_unknown", Some(&venue.0), &text)
                .await;
            let t = format!("Площадка {} на паузе: {why}", venue.0);
            deps.notify("risk.stop", Some(&venue.0), &t).await;
        }
        return Next::Stop;
    }
    if let Some(code) = event_of(err.code) {
        deps.notify(code, Some(&venue.0), &text).await;
    }
    if let Some(paused) = risk::on_error(core, venue) {
        let t = format!("Площадка {} на паузе: {paused}", venue.0);
        deps.notify("risk.stop", Some(&venue.0), &t).await;
    }
    let rules = match crate::storage::rules::list(&deps.db).await {
        Ok(r) => r,
        Err(e) => {
            deps.log(
                Level::Error,
                "core.rules",
                Some(&venue.0),
                format!("Правила шлюза не прочитаны: {e} — операция остановлена"),
                None,
            )
            .await;
            return Next::Stop;
        }
    };
    let Some(rule) = pick_rule(&rules, err.code, Some(venue)) else {
        deps.log(
            Level::Warn,
            "core.rules",
            Some(&venue.0),
            format!("Правила для {:?} нет — операция остановлена", err.code),
            None,
        )
        .await;
        return Next::Stop;
    };
    match &rule.action {
        RuleAction::Retry { attempts, pause_ms } if attempt <= *attempts => Next::Retry {
            pause_ms: *pause_ms,
        },
        RuleAction::StopVenue => {
            let why = format!("правило шлюза №{}: {:?}", rule.id, err.code);
            risk::on_error(core, venue);
            super::engine::lk(&core.inner.risk)
                .venue_paused
                .insert(venue.clone(), why.clone());
            deps.notify(
                "risk.stop",
                Some(&venue.0),
                &format!("Площадка {} остановлена: {why}", venue.0),
            )
            .await;
            Next::Stop
        }
        RuleAction::StopAll => {
            let why = format!("правило шлюза №{}: {:?} на {}", rule.id, err.code, venue.0);
            let t = risk::set_mode(core, Mode::Stopped, Some(why));
            deps.notify("risk.stop", Some(&venue.0), &t).await;
            Next::Stop
        }
        // CancelNoRetry, NotifyOnly, исчерпанные повторы: результат операции не получен — дальше не идём.
        _ => Next::Stop,
    }
}
