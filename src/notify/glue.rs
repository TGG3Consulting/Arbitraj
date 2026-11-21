//! Файл: notify/glue.rs
//! Что делает: решение 251, ответ (г) Тиграна 10.11 — события связи площадки (GLUED: потеря и восстановление связи,
//! лимит запросов, зависшие квитанции узла, блок соединений потока) склеиваются по площадке: первое событие уходит
//! сразу; пока оно отправляется, следующие события этой площадки копятся и уходят следующим ОДНИМ сообщением (и так
//! дальше, пока копится). Окна ожидания (числа) нет — как склейка изменений настроек (notify/settings_watch.rs).
//! Каждое событие пачки проходит свои фильтры (notify.event.enabled / venues / condition — Notifier::allows); одно
//! прошедшее — обычным сообщением (Notifier::deliver), несколько — одним: заголовок «<площадка>: события связи — N»,
//! дальше текст каждого (без технического хвоста «Подробно»; полный текст каждого — в журнал, data.texts).
//! Ответ 429 — одна повторная попытка ровно через parameters.retry_after
//! (ResponseParameters, https://core.telegram.org/bots/api#responseparameters).
//! Отправка идёт своей задачей: вызывающий (ядро, адаптер) Telegram не ждёт.
//! Память очереди — на процесс (одна на бота), по id площадки.
//! Кто вызывает: notify/dispatch.rs (Notifier::send).
//! От чего зависит: notify/dispatch.rs (deliver, allows, body), notify/catalog.rs, notify/client.rs, notify/alert.rs,
//! notify/human.rs (Msg, имя площадки), notify/tg_html.rs, notify/tg_methods.rs, storage/journal.
//! Настройки: нет своих (фильтры событий — notify.event.*; чат «Общий» — notify.telegram_chat).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde_json::json;

use crate::storage::journal::Level;

use super::alert::{DETAILS, telegram};
use super::dispatch::{Notifier, body};
use super::human::{Msg, WARN, venue as venue_name};
use super::{catalog, client, tg_methods};

/// Коды событий, которые склеиваются по площадке (notify/catalog.rs, группа «Площадки» и dex.receipts_stuck).
pub static GLUED: &[&str] = &[
    "venue.disconnected",
    "venue.reconnected",
    "venue.rate_limited",
    "dex.receipts_stuck",
    "venue.feed_conn_blocked",
];

/// Событие в очереди площадки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub code: String,
    pub venue: Option<String>,
    pub text: String,
}

/// Очередь одной площадки: идёт ли отправка и что накопилось за неё.
#[derive(Debug, Default)]
struct Slot {
    busy: bool,
    queue: Vec<Item>,
}

/// Склеивается ли событие. Принимает: код. Ошибок нет.
pub fn glued(code: &str) -> bool {
    GLUED.contains(&code)
}

/// Очереди площадок (на процесс). Отравленный замок — данные как есть (частичной записью не ломаются).
fn slots() -> MutexGuard<'static, HashMap<String, Slot>> {
    static S: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    S.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Событие пришло. Принимает: ключ площадки, событие. Возвращает: Some(событие) — отправки по площадке нет, вызывающий
/// начинает её с этого события; None — отправка идёт, событие ждёт в очереди. Ошибок нет.
pub fn enter(key: &str, item: Item) -> Option<Item> {
    let mut m = slots();
    let slot = m.entry(key.to_string()).or_default();
    if slot.busy {
        slot.queue.push(item);
        return None;
    }
    slot.busy = true;
    Some(item)
}

/// Отправка кончилась. Принимает: ключ площадки. Возвращает: накопленное за неё (следующая пачка) или пусто — тогда
/// отправка по площадке завершена (следующее событие начнёт новую). Ошибок нет.
pub fn next(key: &str) -> Vec<Item> {
    let mut m = slots();
    let slot = m.entry(key.to_string()).or_default();
    if slot.queue.is_empty() {
        slot.busy = false;
    }
    std::mem::take(&mut slot.queue)
}

/// Событие связи площадки: сразу в отправку или в очередь (см. шапку). Принимает: отправителя, код, площадку, текст.
/// Ошибок нет.
pub fn push(n: &Notifier, code: &str, venue: Option<&str>, text: &str) {
    let key = venue.unwrap_or_default().to_string();
    let item = Item {
        code: code.to_string(),
        venue: venue.map(str::to_string),
        text: text.to_string(),
    };
    let Some(first) = enter(&key, item) else {
        return;
    };
    let n = n.clone();
    tokio::spawn(async move {
        let mut batch = vec![first];
        while !batch.is_empty() {
            n.send_batch(&batch).await;
            batch = next(&key);
        }
    });
}

/// Текст одного сообщения на пачку (≥ 2 событий): заголовок и текст каждого события без хвоста «Подробно».
/// Принимает: ключ площадки, события. Возвращает: текст без разметки. Ошибок нет.
pub fn batch_text(key: &str, items: &[Item]) -> String {
    let who = if key.is_empty() {
        "Площадки".to_string()
    } else {
        venue_name(key)
    };
    let mut parts =
        vec![Msg::new(WARN, format!("{who}: события связи — {}", items.len())).render()];
    for i in items {
        let Some(def) = catalog::find(&i.code) else {
            continue;
        };
        let full = body(&i.code, def, &i.text);
        let short: Vec<&str> = full.lines().take_while(|l| *l != DETAILS).collect();
        parts.push(short.join("\n").trim_end().to_string());
    }
    parts.join("\n\n")
}

impl Notifier {
    /// Отправка пачки событий площадки: фильтры каждого; одно — обычным сообщением, несколько — одним (batch_text).
    /// Ошибок нет: всё — в журнал.
    async fn send_batch(&self, items: &[Item]) {
        let (ok, rest): (Vec<&Item>, Vec<&Item>) = items
            .iter()
            .partition(|i| self.allows(&i.code, i.venue.as_deref()));
        // Не прошедшие фильтры — через deliver: выключенное пишет в журнал (notify/journal_off.rs), остальные фильтры
        // пропускает молча, по условию пишет причину.
        for i in rest {
            self.deliver(&i.code, i.venue.as_deref(), &i.text, None)
                .await;
        }
        match ok.as_slice() {
            [] => {}
            [one] => {
                let v = one.venue.as_deref();
                if let Some(s) = self.deliver(&one.code, v, &one.text, None).await {
                    tokio::time::sleep(Duration::from_secs(s)).await;
                    self.deliver(&one.code, v, &one.text, None).await;
                }
            }
            many => self.send_many(many).await,
        }
    }

    /// Несколько событий площадки — одним сообщением в «Общий». Ошибок нет: итог — в журнал.
    async fn send_many(&self, items: &[&Item]) {
        let venue = items.first().and_then(|i| i.venue.clone());
        let key = venue.clone().unwrap_or_default();
        let owned: Vec<Item> = items.iter().map(|i| (*i).clone()).collect();
        let html = telegram(&batch_text(&key, &owned));
        let ready = match client::build(&self.http, &self.settings, &self.vault) {
            Ok(r) => r,
            Err(e) => {
                let why = format!(
                    "События связи ({}) не отправлены: {}",
                    items.len(),
                    e.message
                );
                return self.log(Level::Warn, venue.as_deref(), why).await;
            }
        };
        let mut res = ready.tg.send_html(&ready.chat, &html, None).await;
        if let Some(s) = res.as_ref().err().and_then(tg_methods::retry_after) {
            tokio::time::sleep(Duration::from_secs(s)).await;
            res = ready.tg.send_html(&ready.chat, &html, None).await;
        }
        let codes: Vec<&str> = items.iter().map(|i| i.code.as_str()).collect();
        let texts: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
        let (level, line) = match res {
            Ok(_) => (
                Level::Info,
                format!(
                    "Отправлено в Telegram одним сообщением: событий связи — {}",
                    items.len()
                ),
            ),
            Err(e) => (
                Level::Error,
                format!("События связи ({}) не отправлены: {e}", items.len()),
            ),
        };
        let data = json!({ "codes": codes, "texts": texts });
        let _ = self
            .journal
            .write(level, "notify", venue.as_deref(), line, Some(data))
            .await;
    }
}
