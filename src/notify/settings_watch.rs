//! Файл: notify/settings_watch.rs
//! Что делает: слушает изменения настроек на лету и шлёт событие «system.settings_changed»
//! (кто, что и когда поменял). Журнал изменений пишет само хранилище (settings/store.rs),
//! поэтому медленный Telegram не может привести к потере записей журнала.
//! Решение 251 (Тигран 10.11: согласовано): пачка изменений — ОДНО сообщение со списком. 09.11 19:12 каждое изменение
//! пачки уходило своим сообщением, и Telegram ответил 429. Теперь: первое изменение + все, что уже пришли; пока
//! сообщение отправляется, новые изменения копятся и уходят следующим одним сообщением; 429 — повтор ровно через
//! retry_after (Notifier::send_retry). Окна ожидания (числа) нет: склеивается то, что пришло, пока шла отправка.
//! Кто вызывает: main.rs (запускает задачу).
//! От чего зависит: settings/live.rs (Watch::next, try_next), notify/dispatch.rs, notify/human.rs (Msg).
//! Настройки: событие system.settings_changed включается в интерфейсе.

use crate::settings::live::{Change, Watch};
use crate::settings::value::Scope;

use super::dispatch::Notifier;
use super::human::{INFO, Msg};

/// Код события в notify/catalog.rs.
const EVENT: &str = "system.settings_changed";

/// Работает, пока система запущена. Принимает: подписку на все изменения и отправителя уведомлений.
/// Ошибок не возвращает.
pub async fn run(mut watch: Watch, notifier: Notifier) {
    while let Some(first) = watch.next().await {
        let mut batch = vec![first];
        while let Some(c) = watch.try_next() {
            batch.push(c);
        }
        notifier.send_retry(EVENT, None, &text(&batch)).await;
    }
}

/// Текст одного сообщения на пачку изменений: заголовок с числом, «Кто» (если один), по строке на изменение.
/// Принимает: изменения (не пусто). Возвращает: текст по шаблону (notify/human_msg.rs). Ошибок нет.
pub fn text(batch: &[Change]) -> String {
    let head = match batch.len() {
        1 => "Изменена настройка".to_string(),
        n => format!("Изменены настройки: {n}"),
    };
    let one_by = batch
        .iter()
        .all(|c| batch.first().is_some_and(|f| f.by == c.by));
    let mut m = Msg::new(INFO, head);
    if one_by && let Some(f) = batch.first() {
        m = m.kv("Кто", &f.by);
    }
    for c in batch {
        let who = if one_by {
            String::new()
        } else {
            format!(" (кто: {})", c.by)
        };
        m = m.line(format!(
            "• {}{}: {} → {}{who}",
            c.name,
            scope(&c.scope),
            shown(&c.old),
            shown(&c.new)
        ));
    }
    m.render()
}

/// Область для человека: общая — пусто, иначе « [площадка bingx]» и т.п. Ошибок нет.
fn scope(s: &Scope) -> String {
    match s {
        Scope::Global => String::new(),
        other => format!(" [{}]", other.to_key()),
    }
}

/// Значение для человека: строка — без кавычек JSON, не задано — «не задано». Ошибок нет.
fn shown(v: &Option<serde_json::Value>) -> String {
    match v {
        None => "не задано".into(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(x) => x.to_string(),
    }
}
