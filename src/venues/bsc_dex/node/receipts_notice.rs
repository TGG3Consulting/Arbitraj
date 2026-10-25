//! Файл: venues/bsc_dex/node/receipts_notice.rs
//! Что делает: уведомление ядру от ленты квитанций (событие Notice → Telegram по коду, например dex.receipts_stuck —
//! решение 72) без молчаливой потери (ревью #62): канал полон — отправка отдельной задачей, когда место появится;
//! канала нет или он закрыт — запись в журнал, что событие не ушло.
//! Кто вызывает: node/receipts_lag.rs (overdue).
//! От чего зависит: receipts.rs (Feed: канал событий ядру), ctx.rs.
//! Настройки: нет.

use super::super::ctx::Ctx;
use super::receipts::{Feed, SOURCE};

impl Feed {
    /// Уведомление ядру (Telegram по событию `code`) без молчаливой потери: канал полон — отправка отдельной задачей,
    /// когда место появится; канала нет или он закрыт — запись в журнал, что событие не ушло. Ошибок нет.
    pub(super) async fn notice(&self, ctx: &Ctx, code: &str, text: String) {
        use tokio::sync::mpsc::error::TrySendError;
        let out = self
            .inner
            .out
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let ev = crate::gateway::events::MarketEvent::Notice {
            venue: ctx.venue(),
            code: code.into(),
            text,
        };
        let lost = match out {
            None => Some("канал событий ядру ещё не открыт".to_string()),
            Some(out) => match out.try_send(ev) {
                Ok(()) => None,
                Err(TrySendError::Full(ev)) => {
                    let ctx = ctx.clone();
                    let code = code.to_string();
                    tokio::spawn(async move {
                        if out.send(ev).await.is_err() {
                            let msg = format!(
                                "{SOURCE}: уведомление {code} не ушло — канал событий ядру закрыт"
                            );
                            ctx.log(crate::storage::journal::Level::Warn, "venue.ws", msg, None)
                                .await;
                        }
                    });
                    None
                }
                Err(TrySendError::Closed(_)) => Some("канал событий ядру закрыт".to_string()),
            },
        };
        if let Some(why) = lost {
            let msg = format!("{SOURCE}: уведомление {code} не ушло — {why}");
            ctx.log(crate::storage::journal::Level::Warn, "venue.ws", msg, None)
                .await;
        }
    }
}
