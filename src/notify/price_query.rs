//! Telegram price check for a position still awaiting a sale decision.
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::core::Core;
use crate::core::types::{ManualRecord, RecordKind};
use crate::storage::journal::{Journal, Level};

use super::mirror_state::{MirrorState, Pending, Shared, lock};
use super::press::Ctx;
use super::press_record::ALREADY;
use super::telegram::Telegram;
use super::tg_buttons::{CallbackQuery, CbMessage};

pub(super) fn eligible(record: &ManualRecord) -> bool {
    matches!(record.kind, RecordKind::Alert | RecordKind::Halted)
        && record
            .button("sell_ret")
            .is_some_and(|b| b.disabled.is_none())
}

// Deal IDs can contain long UTF-8 tickers. Look up the reference among live records;
// the buttons that move funds subsequently use server-side, single-use confirmations.
fn reference(key: &str) -> String {
    hex::encode(&Sha256::digest(key.as_bytes())[..16])
}

pub(super) fn button(key: &str, prefix: &str) -> Value {
    json!({ "text": format!("{prefix}Запрос цены"), "style": "danger",
        "callback_data": format!("pq:{}", reference(key)) })
}

impl Ctx<'_> {
    pub(super) async fn price_query(&self, q: &CallbackQuery, msg: &CbMessage, object: &str) {
        let mut matches = self
            .core
            .manual_records()
            .into_iter()
            .filter(|r| eligible(r) && reference(&r.key) == object);
        let Some(record) = matches.next() else {
            return self.answer(&q.id, Some(ALREADY)).await;
        };
        if matches.next().is_some() {
            return self.answer(&q.id, Some("Неоднозначный номер записи")).await;
        }
        let chat = msg.chat.id;
        let read_key = (chat, record.key.clone());
        let admitted = {
            let mut state = lock(self.state);
            !state.done.contains(&record.key) && state.price_reads.insert(read_key.clone())
        };
        if !admitted {
            return self.answer(&q.id, Some("Запись уже обрабатывается")).await;
        }
        self.answer(&q.id, Some("Запрашиваю цены")).await;
        let (core, tg, state, journal) = (
            self.core.clone(),
            self.tg.clone(),
            self.state.clone(),
            self.journal.clone(),
        );
        let ttl = self.ttl();
        let message = msg.message_id;
        // Quoting exchanges can take several seconds. Keep getUpdates available to all chats.
        tokio::spawn(async move {
            let out = QuoteMessage {
                tg: &tg,
                state: &state,
                journal: &journal,
                chat,
                message,
            };
            out.send(&core, &record.key, ttl).await;
            lock(&state).price_reads.remove(&read_key);
        });
    }
}

struct QuoteMessage<'a> {
    tg: &'a Telegram,
    state: &'a Shared,
    journal: &'a Journal,
    chat: i64,
    message: i64,
}

impl QuoteMessage<'_> {
    async fn send(&self, core: &Core, key: &str, ttl: Result<Duration, String>) {
        let mut choices = Vec::new();
        let mut text = match core.sale_price_query(key).await {
            Err(reason) => format!("Запрос цены: {reason}"),
            Ok(quote) => {
                if let (Some(action), Some(to)) = (quote.away, quote.best_venue) {
                    choices.push((format!("Перегнать и продать · {to}"), action));
                }
                if let Some(action) = quote.here {
                    let title = if quote.at_origin {
                        "Продать на тек. месте"
                    } else {
                        "Про. Перегн. на тек. месте"
                    };
                    choices.push((title.to_string(), action));
                }
                quote.text
            }
        };
        let chat = self.chat.to_string();
        let mut ids = Vec::new();
        let mut rows = Vec::new();
        if !choices.is_empty() {
            match ttl {
                Ok(ttl) => {
                    text.push_str("\n\nНажатие кнопки запускает выбранную операцию.");
                    let mut state = lock(self.state);
                    state.purge_pending(ttl);
                    for (title, action) in choices {
                        let n = state.add_pending(Pending {
                            key: Some(key.to_string()),
                            action,
                            title: title.clone(),
                            question: text.clone(),
                        });
                        ids.push(n);
                        rows.push(json!([{ "text": title, "callback_data": format!("y:{n}") }]));
                    }
                }
                Err(why) => text.push_str(&format!("\n\n{why}")),
            }
        }
        let markup = (!rows.is_empty()).then(|| json!({ "inline_keyboard": rows }));
        match self
            .tg
            .send_markup(&chat, &text, markup, Some(self.message))
            .await
        {
            Ok(sent) => {
                let mut state = lock(self.state);
                for n in ids {
                    state.pending_at.insert(n, (chat.clone(), sent));
                }
            }
            Err(e) => {
                {
                    let mut state = lock(self.state);
                    for n in ids {
                        state.pending.remove(&n);
                        state.pending_born.remove(&n);
                    }
                }
                let _ = self
                    .journal
                    .write(
                        Level::Warn,
                        "notify",
                        None,
                        format!("Ответ на запрос цены записи {key} не отправлен: {e}"),
                        None,
                    )
                    .await;
            }
        }
    }
}

/// Claiming a button also consumes the other choice attached to that quote message.
pub(super) fn clear_other_choices(state: &mut MirrorState, selected: u64) {
    let Some(owner) = state.pending_at.get(&selected) else {
        return;
    };
    let siblings: Vec<_> = state
        .pending_at
        .iter()
        .filter(|(n, at)| **n != selected && *at == owner)
        .map(|(n, _)| *n)
        .collect();
    for n in siblings {
        state.pending_at.remove(&n);
        state.pending_born.remove(&n);
        state.pending.remove(&n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::ManualAction;

    #[test]
    fn callback_fits_for_unicode_and_address_ids() {
        let key = format!("d.20261125000000123{}", "山野万里".repeat(12));
        let b = button(&key, "");
        let data = b["callback_data"].as_str().unwrap();
        assert!(data.len() <= super::super::callback::CALLBACK_DATA_MAX_BYTES);
        assert_eq!(data, format!("pq:{}", reference(&key)));
        assert_ne!(reference(&key), reference(&format!("{key}2")));
        assert_eq!(b["style"], "danger");
    }

    #[test]
    fn a_choice_consumes_only_its_own_quote_message() {
        let mut state = MirrorState::default();
        let mut add = |chat: &str, message, away| {
            let n = state.add_pending(Pending {
                key: Some("a.1".into()),
                action: ManualAction::QuotedSale {
                    record: "a.1".into(),
                    quote_id: "qid".into(),
                    away,
                },
                title: String::new(),
                question: String::new(),
            });
            state.pending_at.insert(n, (chat.into(), message));
            n
        };
        let here = add("-100", 10, false);
        let away = add("-100", 10, true);
        let other_message = add("-100", 11, true);
        let other_chat = add("-200", 10, false);
        assert!(
            !state.busy("a.1"),
            "requesting a price must not silence reminders"
        );
        clear_other_choices(&mut state, here);
        assert!(!state.pending.contains_key(&away));
        assert!(!state.pending_born.contains_key(&away));
        assert!(!state.pending_at.contains_key(&away));
        for kept in [here, other_message, other_chat] {
            assert!(state.pending.contains_key(&kept));
        }
    }
}
