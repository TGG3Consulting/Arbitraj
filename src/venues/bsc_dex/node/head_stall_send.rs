//! Файл: venues/bsc_dex/node/head_stall_send.rs
//! Что делает: решение 282 — отправка события dex.node_stalled / dex.node_resumed ядру (MarketEvent::Notice) без
//! молчаливой потери; вынесено из node/head_stall.rs (ревью t122: файл сверх 200 строк). «Не ушло» пишется в журнал
//! только по просьбе вызывающего — один раз на переход (head_stall.rs State.said).
//! Кто вызывает: node/head_stall.rs (step_with).
//! От чего зависит: ctx.rs, gateway/events.rs (MarketEvent), storage/journal.
//! Настройки: нет.

use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;

/// Событие ядру без молчаливой потери (как node/receipts_notice.rs): канал полон — отдельной задачей; канала нет
/// или закрыт — запись в журнал, что событие не ушло (только при log_lost: первая попытка перехода). Принимает:
/// канал ядру (снимок Stall.out), окружение, код, текст, писать ли «не ушло». Возвращает: true — ушло в канал или
/// встало в очередь отдельной задачей; false — не ушло (канала нет / закрыт). Ошибок нет.
pub async fn send(
    out: Option<mpsc::Sender<MarketEvent>>,
    ctx: &Ctx,
    code: &str,
    text: String,
    log_lost: bool,
) -> bool {
    use tokio::sync::mpsc::error::TrySendError;
    let ev = MarketEvent::Notice {
        venue: ctx.venue(),
        code: code.into(),
        text,
    };
    let lost = match out {
        None => Some("канал событий ядру ещё не открыт"),
        Some(out) => match out.try_send(ev) {
            Ok(()) => None,
            Err(TrySendError::Full(ev)) => {
                let (ctx, code) = (ctx.clone(), code.to_string());
                tokio::spawn(async move {
                    if out.send(ev).await.is_err() {
                        let msg = format!("Уведомление {code} не ушло — канал событий ядру закрыт");
                        ctx.log(Level::Warn, "venue.head_stall", msg, None).await;
                    }
                });
                None
            }
            Err(TrySendError::Closed(_)) => Some("канал событий ядру закрыт"),
        },
    };
    if let Some(why) = lost {
        if log_lost {
            let msg = format!("Уведомление {code} не ушло — {why}");
            ctx.log(Level::Warn, "venue.head_stall", msg, None).await;
        }
        return false;
    }
    true
}
