//! Файл: venues/gate/withdraw_notice.rs
//! Что делает: событие «вывод Gate в статусе FAIL» (решение 135, Тигран 04.11): FAIL («On-Chain Failure Pending
//! Confirmation») — не провал вывода, бот ждёт следующего статуса (transfers.rs), но сразу пишет в журнал и шлёт
//! событие уведомлений exec.withdraw_fail_pending (MarketEvent::Notice → ядро → notify, включается на экране
//! «Уведомления»). Один раз на вывод: статус опрашивается многократно, повторов в Telegram нет.
//! Кто вызывает: transfers.rs (withdrawal_status), mod.rs (канал событий — с подпиской на рынок).
//! От чего зависит: gateway/events.rs (MarketEvent::Notice), storage/journal.rs, ctx.rs, num.rs.
//! Настройки: нет своих (включение события — notify.event.* с областью «событие», код — notify/catalog.rs).
//!
//! Официальная документация: API v4 → Wallet → «Retrieve withdrawal records» GET /wallet/withdrawals
//! https://www.gate.com/docs/developers/apiv4/en/ (SDK: docs/WithdrawalRecord.md) — status FAIL «On-Chain Failure
//! Pending Confirmation», поля id, currency, amount, chain, txid, fail_reason.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::ctx::{Ctx, ID};
use super::num;

/// Код события (notify/catalog.rs, группа «Ошибки исполнения»).
pub const FAIL_PENDING: &str = "exec.withdraw_fail_pending";

/// Канал событий ядра и выводы, о которых уже сообщено. Дешёво клонируется.
#[derive(Clone, Default)]
pub struct Notices {
    out: Arc<Mutex<Option<mpsc::Sender<MarketEvent>>>>,
    seen: Arc<Mutex<HashSet<String>>>,
}

impl Notices {
    /// Запоминает канал событий ядра (из подписки на рынок). Ошибок нет.
    pub fn set_out(&self, out: mpsc::Sender<MarketEvent>) {
        *self.out.lock().unwrap_or_else(|p| p.into_inner()) = Some(out);
    }

    /// Что делает: вывод id в статусе FAIL — запись в журнал и событие, один раз на вывод.
    /// Принимает: окружение, ID вывода Gate, запись истории вывода. Ошибок нет: нет канала событий — только журнал
    /// (с пометкой), полный канал — событие теряется, журнал остаётся.
    pub async fn fail_pending(&self, ctx: &Ctx, id: &str, rec: &Value) {
        if !self
            .seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.to_string())
        {
            return;
        }
        let text = fail_text(id, rec);
        let out = self.out.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let sent = out.is_some_and(|o| {
            o.try_send(MarketEvent::Notice {
                venue: ctx.venue(),
                code: FAIL_PENDING.to_string(),
                text: text.clone(),
            })
            .is_ok()
        });
        let note = if sent {
            ""
        } else {
            " (событие не отправлено: нет канала событий ядра — подписка на рынок Gate не запущена)"
        };
        let data = json!({ "withdraw_id": id, "record": rec });
        let _ = ctx
            .journal
            .write(
                Level::Warn,
                "venue.transfer",
                Some(ID),
                format!("{text}{note}"),
                Some(data),
            )
            .await;
    }
}

/// Текст события: монета, сумма, сеть, хеш и причина из записи. Чистая. Ошибок нет.
pub fn fail_text(id: &str, rec: &Value) -> String {
    let f = |k: &str| num::text_opt(rec, k).unwrap_or_else(|| "—".into());
    format!(
        "Gate: вывод {id} ({} {}, сеть {}) — статус FAIL «On-Chain Failure Pending Confirmation»: не провал, бот ждёт \
         следующего статуса. txid {}, причина: {}",
        f("amount"),
        f("currency"),
        f("chain"),
        f("txid"),
        f("fail_reason")
    )
}
