//! Файл: core/rules_apply.rs
//! Что делает: применение правил шлюза к ошибке операции ядра (CLAUDE.md, «Архитектура»): правило
//! площадки сильнее общего (gateway/rules.rs, pick_rule); правила вводит Тигран в интерфейсе, в коде их нет.
//! Стартовое правило Тиграна «ордер не исполнился — отменяем, больше не пробуем» — это CancelNoRetry.
//! Правила нет — операция останавливается и уведомление уходит (gateway/README.md).
//! Каждая ошибка — в журнал и в уведомление своего события; ошибки подряд считает risk.rs.
//! Решение 177 (risk_count.rs): отказ до отправки (деньги и газ не тронуты) ошибкой площадки подряд не считается; ручная
//! команда человека не считается совсем, и правило StopVenue паузу по ней не ставит — паузу ставит только автоматика.
//! «Исход неизвестен» ставит паузу и у ручной команды: транзакция могла исполниться. Отказ остановленной очереди
//! кошелька (QueueHalted, решение 231 — дополнение) — не «исход неизвестен»: транзакция не подписывалась, обычные правила.
//! Решение 220 (Тигран 08.11): «вывод отклонён» (WithdrawRejected) не повторяется при любых правилах — журнал
//! core.withdraw_rejected «вывод отклонён — не повторяем». Решение 230: правило Retry не действует и для InvalidAddress,
//! WithdrawLocked (gateway/rules.rs withdraw_refusal; у WithdrawLocked — свой повтор, решение 166).
//! Решение 221: пауза площадки отсюда (StopVenue, «исход неизвестен») снимает отметку сверки — автоснятия сверкой нет
//! (rules_stop.rs, forget_reconcile).
//! Решение 225: паузы отсюда — софтовые (auto_pause.rs): «стоп площадка», «стоп всё» и ошибки подряд снимает сам бот
//! проверкой подключения (auto_resume.rs), «исход неизвестен» — только человек; отказ вывода / неверный адрес / вывод
//! заблокирован паузу не ставят вовсе. Постановка пауз — rules_stop.rs.
//! Решение 253, дополнение t85: исчерпан лимит вывода аккаунта (WithdrawLimit) — пауза площадки до ручного снятия и
//! алерт venue.withdraw_limit (withdraw_limit_pause.rs; у ручной команды тоже), правила шлюза — как у отказа вывода.
//! Решение 268: «цена ордера вне диапазона биржи» (PriceOutOfRange) правилом Retry не повторяется; своего правила для
//! кода нет — действует правило OrderRejected (gateway/rules.rs pick_rule_or_fallback), запись core.rules об этом.
//! Кто вызывает: executor.rs, transfers.rs, stuck.rs, usdt_return.rs.
//! От чего зависит: gateway (errors, rules), storage/rules.rs, risk.rs, risk_count.rs, rules_stop.rs,
//! withdraw_limit_pause.rs, deps.rs.
//! Настройки: правила шлюза (SQLite), risk.max_errors_in_row (через risk.rs), risk.auto_resume_check_s и
//! risk.errors_resume_after_s (текст уведомления, rules_stop.rs).

use serde_json::json;

use crate::core::types::VenueId;
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::gateway::rules::RuleAction;
use crate::storage::journal::Level;

use crate::notify::human::{self as h, Msg};

use super::auto_pause::Kind;
use super::engine::Core;
use super::risk;
use super::rules_stop::{self as stop, paused_text};

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
        // Решение 225: паузы по ним нет — только предупреждение (журнал и это событие).
        ErrorCode::WithdrawRejected
        | ErrorCode::WithdrawLocked
        | ErrorCode::InvalidAddress
        | ErrorCode::AccountRestricted => "exec.withdraw_rejected",
        // Дополнение t85: лимит вывода — свой алерт venue.withdraw_limit (withdraw_limit_pause.rs), без этого дубля.
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
    // любых правилах шлюза, площадка — на паузу до ручной проверки (rules_stop.rs; снимает только человек).
    if !crate::gateway::rules::retry_allowed(err.code) {
        stop::outcome(core, op, venue, deal, err).await;
        return Next::Stop;
    }
    // Решение 253, дополнение t85: исчерпан лимит вывода аккаунта — площадка на паузу до ручного снятия, алерт.
    if err.code == ErrorCode::WithdrawLimit {
        super::withdraw_limit_pause::pause(core, op, venue, deal, err).await;
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
        let note = stop::resume_note(core, venue, Kind::Errors);
        deps.notify(
            "risk.stop",
            Some(&venue.0),
            &paused_text(venue, &paused, note),
        )
        .await;
    }
    // Решение 220 (Тигран 08.11): вывод отклонён площадкой (WithdrawRejected: Bitget 40938 «адресная книга» и т. п.) —
    // тот же вывод не повторяем при любых правилах шлюза (Retry ниже не действует); вместо повтора — продажа на месте
    // (relay_here.rs) или «USDT не вернулись» (usdt_return_retry.rs).
    let rejected = err.code == ErrorCode::WithdrawRejected;
    if rejected {
        deps.log(
            Level::Warn,
            "core.withdraw_rejected",
            Some(&venue.0),
            format!("{op} на {}: вывод отклонён — не повторяем", venue.0),
            Some(json!({ "deal": deal })),
        )
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
    // Решение 268 (дополнение): своего правила для PriceOutOfRange нет — действует правило OrderRejected (как до 268).
    let picked = crate::gateway::rules::pick_rule_or_fallback(&rules, err.code, Some(venue));
    if let Some((r, true)) = picked {
        deps.log(
            Level::Info,
            "core.rules",
            Some(&venue.0),
            format!(
                "Правила для {:?} нет — применено правило шлюза №{} для OrderRejected (повтор по этому коду не действует, решение 268)",
                err.code, r.id
            ),
            None,
        )
        .await;
    }
    let Some(rule) = picked.map(|(r, _)| r) else {
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
        // Решение 220: WithdrawRejected не повторяется даже правилом Retry (запись в журнале — выше). Решение 230: так же
        // InvalidAddress и WithdrawLocked (у него — только свой повтор, решение 166); форма и API такое правило не
        // сохраняют, сохранённое раньше помечено «не действует».
        RuleAction::Retry { .. } if crate::gateway::rules::withdraw_refusal(err.code) => Next::Stop,
        // Решение 268: цена ордера вне диапазона биржи — тот же ордер не повторяется и правилом Retry.
        RuleAction::Retry { .. } if crate::gateway::rules::price_range(err.code) => Next::Stop,
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
            stop::stop_venue(core, rule.id, venue, err).await;
            Next::Stop
        }
        RuleAction::StopAll => {
            stop::stop_all(core, rule.id, op, venue, deal, err).await;
            Next::Stop
        }
        // CancelNoRetry, NotifyOnly, исчерпанные повторы: результат операции не получен — дальше не идём.
        _ => Next::Stop,
    }
}

/// Решение 164: текст ошибки операции для Telegram — заголовок события, где и что делали, человеческая причина,
/// неизвестный ответ биржи — последней строкой, id сделки. Принимает: код события, операцию, площадку, сделку, ошибку.
/// Публична для теста шаблона алерта (решение 251, tests/notify_alert_251.rs).
pub fn tg_error(
    code: &str,
    op: &str,
    venue: &VenueId,
    deal: Option<&str>,
    err: &GatewayError,
) -> Msg {
    let head = crate::notify::heads::head(code, "");
    let (icon, title) = head.split_once(' ').unwrap_or((h::WARN, head.as_str()));
    Msg::new(icon, title)
        .line(format!("{}: {}", h::venue(&venue.0), lower(op)))
        .reason(&err.message)
        .id(deal.unwrap_or_default())
}

/// Первая буква — строчная (операция в середине строки). Принимает: текст.
pub(super) fn lower(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
