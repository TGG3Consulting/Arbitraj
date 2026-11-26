//! Доставка по одному экземпляру каждой проблемной записи в каждый настроенный чат.
//! В SQLite хранится подтверждение Telegram. Ошибка одного чата не отменяет отправку в остальные.
use super::incident::{Incident, Thresholds};
use crate::notify::dispatch::Notifier;
use crate::notify::outbound::{Channel, build};
use crate::notify::tg_methods::retry_after;
use crate::storage::db::{Db, DbError};
use crate::storage::journal::Level;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

pub(super) async fn recipients(db: &Db, key: &str) -> Result<HashSet<String>, DbError> {
    let key = key.to_owned();
    db.call(move |c| {
        let mut s = c.prepare("SELECT chat FROM notify_duty_sent WHERE incident=?1")?;
        s.query_map([key], |r| r.get(0))?.collect()
    })
    .await
}

pub(super) async fn confirm(db: &Db, key: &str, chat: &str) -> Result<(), DbError> {
    let values = [key.to_owned(), chat.to_owned()];
    db.call(move |c| {
        c.execute(
            "INSERT OR IGNORE INTO notify_duty_sent(incident,chat) VALUES (?1,?2)",
            values,
        )
        .map(|_| ())
    })
    .await
}

#[derive(Default)]
pub(super) struct Delivery {
    // Уже принятые Telegram сообщения, которые пока не удалось записать в SQLite.
    uncommitted: HashSet<(String, String)>,
    retry: HashMap<String, Instant>,
    unavailable: Option<String>,
}

impl Delivery {
    async fn failed_config(&mut self, n: &Notifier, why: String) {
        if self.unavailable.as_ref() != Some(&why) {
            n.log(
                Level::Warn,
                None,
                format!("Дежурный: доставка ожидает — {why}"),
            )
            .await;
            self.unavailable = Some(why);
        }
    }

    pub async fn flush(&mut self, db: &Db, n: &Notifier, pending: &mut BTreeMap<String, Incident>) {
        // Сохраняем уже полученные подтверждения даже при блокировке хранилища токенов.
        for (key, chat) in self.uncommitted.clone() {
            if confirm(db, &key, &chat).await.is_ok() {
                self.uncommitted.remove(&(key, chat));
            }
        }
        let chats = match build(&n.http, &n.settings, &n.vault, Channel::Duty) {
            Ok((_, chats)) => chats,
            Err(e) => {
                self.failed_config(n, e.message).await;
                return;
            }
        };
        self.unavailable = None;
        self.retry
            .retain(|chat, until| chats.contains(chat) && *until > Instant::now());
        let mut completed = Vec::new();
        for (key, i) in pending.iter() {
            let previous = match recipients(db, key).await {
                Ok(rows) => rows,
                Err(e) => {
                    n.log(
                        Level::Error,
                        None,
                        format!("Дежурный: история доставки недоступна — {e}"),
                    )
                    .await;
                    break;
                }
            };
            let mut done = true;
            for chat in chats.iter().filter(|chat| !previous.contains(*chat)) {
                let mark = (key.clone(), chat.clone());
                if self.uncommitted.contains(&mark) || self.retry.contains_key(chat) {
                    done = false;
                    continue;
                }
                // Изменение порогов/получателей и блокировка сейфа действуют со следующего запроса.
                let Ok((tg, current)) = build(&n.http, &n.settings, &n.vault, Channel::Duty) else {
                    return;
                };
                if !current.contains(chat) || !Thresholds::read(&n.settings).qualifies(i) {
                    continue;
                }
                match tg.send(chat, &i.message()).await {
                    Ok(()) => {
                        if let Err(e) = confirm(db, key, chat).await {
                            self.uncommitted.insert(mark);
                            done = false;
                            n.log(
                                Level::Error,
                                None,
                                format!(
                                    "Дежурный: {} отправлена, не сохранено подтверждение: {e}",
                                    i.deal
                                ),
                            )
                            .await;
                        }
                    }
                    Err(e) => {
                        let wait = Duration::from_secs(retry_after(&e).unwrap_or(30));
                        let now = Instant::now();
                        self.retry
                            .insert(chat.clone(), now.checked_add(wait).unwrap_or(now));
                        done = false;
                        n.log(
                            Level::Warn,
                            None,
                            format!("Дежурный: {} не доставлена в чат {chat}: {e}", i.deal),
                        )
                        .await;
                    }
                }
            }
            if done {
                completed.push(key.clone());
            }
        }
        for key in completed {
            pending.remove(&key);
        }
    }
}
