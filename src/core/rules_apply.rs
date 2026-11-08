//! Файл: core/rules_apply.rs
//! Что делает: применение правил шлюза к ошибке операции ядра (CLAUDE.md, «Архитектура»): правило
//! площадки сильнее общего (gateway/rules.rs, pick_rule); правила вводит Тигран в интерфейсе, в коде их нет.
//! Стартовое правило Тиграна «ордер не исполнился — отменяем, больше не пробуем» — это CancelNoRetry.
//! Правила нет — операция останавливается и уведомление уходит (gateway/README.md).
//! Каждая ошибка — в журнал и в уведомление своего события; ошибки подряд считает risk.rs.
//! Решение 177 (risk_count.rs): отказ до отправки (деньги и газ не тронуты) ошибкой площадки подряд не считается; ручная
//! команда человека не считается совсем, и правило StopVenue паузу по ней не ставит — паузу ставит только автоматика.
//! «Исход неизвестен» ставит паузу и у ручной команды: очередь кошелька остановлена, транзакция могла исполниться.
//! Решение 221: пауза площадки отсюда (StopVenue, «исход неизвестен») снимает отметку сверки — автоснятия нет.
//! Кто вызывает: executor.rs, transfers.rs, stuck.rs, usdt_return.rs.
//! От чего зависит: gateway (errors, rules), storage/rules.rs, risk.rs, risk_count.rs, deps.rs.
//! Настройки: правила шлюза (SQLite), risk.max_errors_in_row (через risk.rs).

use serde_json::json;

use crate::core::types::VenueId;
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::gateway::rules::{RuleAction, pick_rule};
use crate::storage::journal::Level;

use crate::notify::human::{self as h, Msg};

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
        ErrorCode::WithdrawRejected | ErrorCode::WithdrawLocked => "exec.withdraw_rejected",
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
    if !crate::gateway::rules::retry_allowed(err.code) {
        let why = format!("{op}: исход транзакции неизвестен — нужна ручная проверка");
        let fresh = {
            let mut r = super::engine::lk(&core.inner.risk);
            // Решение 221: пауза стоит от сверки — заменяется этой (причина, уведомление), сама не снимается.
            let ours = risk::forget_reconcile(&mut r, venue);
            if r.venue_paused.contains_key(venue) && !ours {
                false
            } else {
                r.venue_paused.insert(venue.clone(), why.clone());
                true
            }
        };
        if fresh {
            risk::publish(core);
            let tg = tg_error("send.outcome_unknown", op, venue, deal, err)
                .todo(format!(
                    "проверить на {}, прошла ли операция, затем «Снять паузу» площадки",
                    h::venue(&venue.0)
                ))
                .render();
            deps.notify("send.outcome_unknown", Some(&venue.0), &tg)
                .await;
            deps.notify("risk.stop", Some(&venue.0), &paused_text(venue, &why))
                .await;
        }
        return Next::Stop;
    }
    if let Some(code) = event_of(err.code) {
        let tg = tg_error(code, op, venue, deal, err).render();
        deps.notify(code, Some(&venue.0), &tg).await;
    }
    // Решение 177: ручная команда и отказ до отправки — не «ошибка площадки подряд» (risk_count.rs).
    let manual = super::risk_count::is_manual();
    if !manual
        && super::risk_count::counts(err)
        && let Some(paused) = risk::on_error(core, venue)
    {
        deps.notify("risk.stop", Some(&venue.0), &paused_text(venue, &paused))
            .await;
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
        RuleAction::StopVenue if manual => {
            // Решение 177: паузу площадки ставит только автоматика — ручная команда просто останавливается.
            deps.log(
                Level::Warn,
                "core.rules",
                Some(&venue.0),
                format!(
                    "Правило шлюза №{} ({:?}) — пауза площадки: ручная команда «{op}», паузу ставит только автоматика (решение 177)",
                    rule.id, err.code
                ),
                None,
            )
            .await;
            Next::Stop
        }
        RuleAction::StopVenue => {
            let why = format!("правило шлюза №{}: {:?}", rule.id, err.code);
            risk::on_error(core, venue);
            {
                let mut r = super::engine::lk(&core.inner.risk);
                r.venue_paused.insert(venue.clone(), why.clone());
                // Решение 221: пауза правила шлюза — сама не снимается.
                risk::forget_reconcile(&mut r, venue);
            }
            deps.notify("risk.stop", Some(&venue.0), &paused_text(venue, &why))
                .await;
            Next::Stop
        }
        RuleAction::StopAll => {
            let why = format!("правило шлюза №{}: {:?} на {}", rule.id, err.code, venue.0);
            risk::set_mode(core, Mode::Stopped, Some(why.clone()));
            let tg = Msg::new(h::STOP, "Торговля остановлена")
                .line(format!("Ошибка на {}: {op}", h::venue(&venue.0)))
                .reason(&err.message)
                .kv("Сработало", format!("правило шлюза №{}", rule.id))
                .id(deal.unwrap_or_default())
                .render();
            deps.notify("risk.stop", Some(&venue.0), &tg).await;
            Next::Stop
        }
        // CancelNoRetry, NotifyOnly, исчерпанные повторы: результат операции не получен — дальше не идём.
        _ => Next::Stop,
    }
}

/// Решение 164: текст ошибки операции для Telegram — заголовок события, где и что делали, человеческая причина,
/// неизвестный ответ биржи — последней строкой, id сделки. Принимает: код события, операцию, площадку, сделку, ошибку.
fn tg_error(code: &str, op: &str, venue: &VenueId, deal: Option<&str>, err: &GatewayError) -> Msg {
    let head = crate::notify::heads::head(code, "");
    let (icon, title) = head.split_once(' ').unwrap_or((h::WARN, head.as_str()));
    Msg::new(icon, title)
        .line(format!("{}: {}", h::venue(&venue.0), lower(op)))
        .reason(&err.message)
        .id(deal.unwrap_or_default())
}

/// Решение 164: площадка на паузе (risk.stop). Принимает: площадку, причину паузы.
fn paused_text(venue: &VenueId, why: &str) -> String {
    Msg::new(h::STOP, format!("Площадка {} на паузе", h::venue(&venue.0)))
        .reason(why)
        .todo("разобраться с причиной, затем «Снять паузу» на экране «Управление»")
        .render()
}

/// Первая буква — строчная (операция в середине строки). Принимает: текст.
fn lower(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
