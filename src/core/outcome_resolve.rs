//! Файл: core/outcome_resolve.rs
//! Что делает: решение 231 (Тигран: «последняя пауза, которая остаётся ручной») — бот сам выясняет исход операции со
//! статусом «исход неизвестен» (OutcomeUnknown). Ордер или своп (trade_ops.rs) и вывод (transfers.rs), получив
//! OutcomeUnknown с «чем найти» (GatewayError.pending — свой ID ордера / вывода, ID площадки, хеш и nonce транзакции,
//! gateway/pending.rs), после паузы площадки (rules_stop.rs) не сдаются сразу, а каждые risk.outcome_poll_s спрашивают
//! ту же площадку — Venue::resolve_trade / resolve_withdraw (официальный запрос ордера / история выводов / квитанция).
//! Найден окончательный итог — он возвращается вызывающему как обычный результат (сделка идёт дальше по обычной логике:
//! исполнено / частично / не исполнено с фактическими суммами и комиссиями; вывод — обычный опрос статуса и зачисление),
//! операция снимается с учёта площадки (outcome_open.rs): больше открытых нет — пауза «исход неизвестен» снята; журнал
//! core.outcome_resolved и уведомление outcome.resolved. Не найден за risk.outcome_max_s, площадка не умеет искать
//! (NotApplicable) или настройки не заданы — как раньше: сделка остановлена до ручной проверки, пауза «только человек»;
//! журнал core.outcome_unresolved и уведомление outcome.unresolved.
//! Выяснение идёт в задаче сделки (она ждёт итог, как ждала бы ответа площадки); перезапуск во время выяснения — как
//! раньше: сделка на ручной проверке, пауза площадки после перезапуска не восстанавливается (Risk в памяти).
//! Кто вызывает: trade_ops.rs (trade), transfers.rs (withdraw), rules_stop.rs (cfg — текст уведомления), cycle_outcome.rs
//! (poll, done — цикл DEX ↔ DEX, решение 233, дополнение, п. 4).
//! От чего зависит: outcome_open.rs, risk.rs, engine.rs, deps.rs, gateway (contract, pending, errors), settings.
//! Настройки: risk.outcome_poll_s, risk.outcome_max_s (вся система, можно по площадке; без значений по умолчанию).

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::core::types::{
    TradeRequest, TradeResult, Transfer, TxOutcome, VenueId, WithdrawRequest,
};
use crate::gateway::contract::Venue;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::notify::human::{self as h, Msg};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
use super::outcome_open::{self as open, Lift};

/// Сроки выяснения: как часто спрашивать площадку и сколько всего ждать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cfg {
    pub every: Duration,
    pub max: Duration,
}

/// Сроки выяснения площадки: risk.outcome_poll_s и risk.outcome_max_s (обе заданы). Принимает: ядро, площадку.
/// Возвращает: None — хотя бы одна не задана (выясняет человек). Ошибок нет.
pub fn cfg(core: &Core, v: &VenueId) -> Option<Cfg> {
    let s = &core.inner.deps.settings;
    let at = At {
        venue: Some(&v.0),
        coin: None,
    };
    Some(Cfg {
        every: Duration::from_secs(s.integer("risk.outcome_poll_s", at).ok()?),
        max: Duration::from_secs(s.integer("risk.outcome_max_s", at).ok()?),
    })
}

/// Ключ операции из ошибки: «чем найти» есть и не пуст. Ошибок нет.
pub fn key_of(e: &GatewayError) -> Option<String> {
    e.pending
        .as_ref()
        .map(|p| p.key())
        .filter(|k| !k.is_empty())
}

/// Ордер или своп с неизвестным исходом. Принимает: ядро, название операции, сделку, площадку, адаптер (тот же, что
/// ставил ордер — у DEX кошелёк сделки), исходный запрос, ошибку. Возвращает: Some(Ok(итог)) — найден окончательный
/// итог (вызывающий обрабатывает его как ответ на ордер); Some(Err(ошибка)) — найден и отклонён площадкой (итог
/// окончательный: ничего не исполнено); None — не выяснен (как раньше: ручная проверка). Ошибок нет.
pub async fn trade(
    core: &Core,
    op: &str,
    deal: &str,
    v: &VenueId,
    x: &Arc<dyn Venue>,
    req: &TradeRequest,
    e: &GatewayError,
) -> Option<GatewayResult<TradeResult>> {
    let p = e.pending.as_deref()?.clone();
    let found = poll(core, op, deal, v, e, || x.resolve_trade(req.clone(), &p)).await?;
    let line = match &found {
        Ok(r) => format!(
            "{:?}: исполнено {} по {}, ID {}",
            r.status, r.filled_qty, r.avg_price, r.venue_ref
        ),
        Err(x) => format!("отклонён площадкой: {}", x.message),
    };
    done(core, op, deal, v, e, &line).await;
    Some(found)
}

/// Вывод с неизвестным исходом. Принимает: ядро, сделку, площадку-отправителя, её адаптер, исходный запрос, ошибку.
/// Возвращает: Some(Ok(вывод с ID площадки)) — создан (дальше обычный опрос статуса); Some(Err(ошибка)) — найден и
/// отклонён / откатился (итог окончательный, вывода нет); None — не выяснен. Ошибок нет.
pub async fn withdraw(
    core: &Core,
    deal: &str,
    v: &VenueId,
    x: &Arc<dyn Venue>,
    req: &WithdrawRequest,
    e: &GatewayError,
) -> Option<GatewayResult<Transfer>> {
    let p = e.pending.as_deref()?.clone();
    let op = "Вывод";
    let found = poll(core, op, deal, v, e, || x.resolve_withdraw(req.clone(), &p)).await?;
    let line = match &found {
        Ok(t) => format!("вывод создан, ID {}", t.venue_ref),
        Err(x) => format!("вывода нет: {}", x.message),
    };
    done(core, op, deal, v, e, &line).await;
    Some(found)
}

/// Окончательный отказ площадки по найденной операции (ничего не исполнено): ордер отклонён, вывод отклонён,
/// перевод DEX откатился в блоке или транзакция DEX не попала, а её nonce занят нашей отменой в блоке (факт транзакции:
/// исход NotIncluded и хеш отмены — адаптер ставит их только по квитанции отмены; исходная уже не исполнится — как
/// OpError::cancel_confirmed, решение 229). Ошибок нет.
pub fn definitive(e: &GatewayError) -> bool {
    let cancelled = e.code == ErrorCode::TxNotIncluded
        && e.tx
            .as_ref()
            .is_some_and(|t| t.outcome == TxOutcome::NotIncluded && t.cancel_hash.is_some());
    cancelled
        || matches!(
            e.code,
            ErrorCode::OrderRejected
                | ErrorCode::WithdrawRejected
                | ErrorCode::SwapReverted
                // Решение 268: ордер отклонён или снят биржей из-за цены вне её диапазона — ничего не исполнено.
                | ErrorCode::PriceOutOfRange
        )
}

/// Опрос площадки до окончательного итога или срока. Операция должна быть на учёте «выясняет бот» (rules_stop.rs
/// регистрирует её, если сроки заданы и ключ есть). Первый вопрос — через risk.outcome_poll_s (адаптер только что
/// искал сам). Сроки читаются на каждом круге (меняются на лету); стали не заданы — выясняет человек.
/// Ошибки площадки — журнал core.outcome_resolve один раз на смену текста и опрос дальше; NotApplicable — сразу человек;
/// окончательный отказ (definitive) — Some(Err).
pub(super) async fn poll<T, F, Fut>(
    core: &Core,
    op: &str,
    deal: &str,
    v: &VenueId,
    e: &GatewayError,
    ask: F,
) -> Option<GatewayResult<T>>
where
    F: Fn() -> Fut,
    Fut: Future<Output = GatewayResult<Option<T>>>,
{
    let key = key_of(e)?;
    if !open::resolving(&lk(&core.inner.risk), v, &key) {
        return None;
    }
    let deps = &core.inner.deps;
    let started = Instant::now();
    let mut last = String::new();
    let why = loop {
        let Some(c) = cfg(core, v) else {
            break "сроки выяснения (risk.outcome_poll_s, risk.outcome_max_s) больше не заданы"
                .to_string();
        };
        if started.elapsed() >= c.max {
            break format!(
                "за risk.outcome_max_s ({} с) не найден окончательный итог",
                c.max.as_secs()
            );
        }
        tokio::time::sleep(c.every.min(c.max.saturating_sub(started.elapsed()))).await;
        // Человек снял паузу сам (учёт снят) — бот больше не выясняет.
        if !open::resolving(&lk(&core.inner.risk), v, &key) {
            return None;
        }
        match ask().await {
            Ok(Some(t)) => return Some(Ok(t)),
            Ok(None) => {}
            Err(x) if definitive(&x) => return Some(Err(x)),
            Err(x) if x.code == ErrorCode::NotApplicable => break x.message,
            Err(x) if x.message != last => {
                let msg = format!(
                    "{op} {key} на {}: исход пока не выяснен — {}",
                    v.0, x.message
                );
                deps.log(
                    Level::Warn,
                    "core.outcome_resolve",
                    Some(&v.0),
                    msg,
                    Some(json!({ "deal": deal })),
                )
                .await;
                last = x.message;
            }
            Err(_) => {}
        }
    };
    open::unresolved(&mut lk(&core.inner.risk), v, &key);
    let text = format!(
        "{op} {key} на {}: исход не выяснен ботом — {why}; нужна ручная проверка",
        v.0
    );
    deps.log(
        Level::Error,
        "core.outcome_unresolved",
        Some(&v.0),
        &text,
        Some(json!({ "deal": deal, "error": e })),
    )
    .await;
    let tg = Msg::new(h::STOP, "Исход операции бот не выяснил")
        .line(format!("{op} на {}", h::venue(&v.0)))
        .reason(&why)
        .todo(format!(
            "проверить на {}, прошла ли операция, затем «Снять паузу» площадки",
            h::venue(&v.0)
        ))
        .id(deal)
        .render();
    deps.notify("outcome.unresolved", Some(&v.0), &tg).await;
    None
}

/// Исход найден: операция снята с учёта, пауза снята, если открытых больше нет; журнал и уведомление. Принимает: ядро,
/// операцию, сделку, площадку, исходную ошибку, строку итога. Ошибок нет.
pub(super) async fn done(
    core: &Core,
    op: &str,
    deal: &str,
    v: &VenueId,
    e: &GatewayError,
    line: &str,
) {
    let key = key_of(e).unwrap_or_default();
    let lift = open::resolved(&mut lk(&core.inner.risk), v, &key);
    super::risk::publish(core);
    let pause = match &lift {
        Lift::Lifted => "пауза площадки «исход неизвестен» снята".to_string(),
        Lift::Kept { left, manual } => format!(
            "пауза площадки остаётся: ещё выясняется {left}{}",
            if *manual {
                ", есть операция для ручной проверки"
            } else {
                ""
            }
        ),
        Lift::Gone => "паузу площадки уже снял человек".to_string(),
    };
    let deps = &core.inner.deps;
    let text = format!(
        "{op} {key} на {}: исход выяснен ботом — {line}; {pause}",
        v.0
    );
    deps.log(
        Level::Info,
        "core.outcome_resolved",
        Some(&v.0),
        &text,
        Some(json!({ "deal": deal })),
    )
    .await;
    if lift == Lift::Lifted {
        deps.log(
            Level::Info,
            "core.control",
            Some(&v.0),
            format!("Пауза {} снята: {text}", v.0),
            None,
        )
        .await;
    }
    let tg = Msg::new(h::OK, "Исход операции выяснен ботом")
        .line(format!("{op} на {}", h::venue(&v.0)))
        .kv("Итог", line)
        .line(pause)
        .id(deal)
        .render();
    deps.notify("outcome.resolved", Some(&v.0), &tg).await;
}
