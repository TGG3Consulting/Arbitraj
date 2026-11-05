//! Файл: notify/mirror_view.rs
//! Что делает: текст и inline-кнопки записей «Ручного управления» для Telegram (решение 156) — из тех же записей,
//! что рисует экран (core/types/manual_feed.rs): те же кнопки и надписи; недоступная на экране кнопка в Telegram
//! не показывается, её причина — в тексте; кнопка с вводом суммы — та же, ввод через ответ на сообщение (ForceReply).
//! Плюс сообщение-напоминание пачкой: одно сообщение со списком записей и кнопками каждой (лимиты Telegram).
//! Решение 176: в чате «Неудачные» показываются и недоступные кнопки (причина — по нажатию), в «Общем» — как раньше.
//! Кто вызывает: notify/mirror.rs, notify/menu.rs (экран «Ручное управление» меню).
//! От чего зависит: core/types/manual_feed.rs, notify/callback.rs (encode, предел callback_data 64 байта).
//! Настройки: нет.
//!
//! callback_data кнопки записи — «<код кнопки>:<ключ записи>», например «sell_now:a.<id алерта>».
//! InlineKeyboardMarkup: https://core.telegram.org/bots/api#inlinekeyboardmarkup

use serde_json::{Value, json};

use crate::core::types::ManualRecord;

use super::callback;

/// Кнопки записи, которые можно показать, и строки «недоступно: причина» для текста.
/// Принимает: запись, приставку надписи (номер записи в напоминании, «» — без номера).
/// Возвращает: (кнопки JSON, строки причин). Кнопка, чей callback_data не влез в предел, — в причинах.
pub fn buttons(r: &ManualRecord, prefix: &str) -> (Vec<Value>, Vec<String>) {
    buttons_for(r, prefix, false)
}

/// Как buttons; all = true (чат «Неудачные», решение 176: «кнопки всегда есть, пока запись не закрыта») — недоступная
/// кнопка тоже показывается, её причина — в тексте и ответом на нажатие (press_record.rs). Ошибок нет.
pub fn buttons_for(r: &ManualRecord, prefix: &str, all: bool) -> (Vec<Value>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for b in &r.buttons {
        if let Some(why) = &b.disabled {
            notes.push(format!("«{}» недоступна: {why}", b.title));
            if !all {
                continue;
            }
        }
        match callback::encode(&b.code, &r.key) {
            Ok(data) => {
                out.push(json!({ "text": format!("{prefix}{}", b.title), "callback_data": data }))
            }
            Err(e) => notes.push(format!("«{}» — только в интерфейсе: {e}", b.title)),
        }
        if b.input.is_some() {
            notes.push(format!(
                "«{}»: бот спросит сумму — ответьте на его сообщение числом",
                b.title
            ));
        }
    }
    (out, notes)
}

/// Сообщение одной записи: заголовок вида, поля записи, причины недоступности; кнопки — по одной в ряду
/// (длинные надписи не обрезаются).
/// Принимает: запись. Возвращает: (текст, клавиатура или None — показать нечего).
pub fn record_message(r: &ManualRecord) -> (String, Option<Value>) {
    record_message_for(r, false)
}

/// Как record_message; all — показывать и недоступные кнопки (чат «Неудачные», решение 176). Ошибок нет.
pub fn record_message_for(r: &ManualRecord, all: bool) -> (String, Option<Value>) {
    let (btns, notes) = buttons_for(r, "", all);
    // Решение 164: текст записи уже со своим заголовком и значком (core/manual_rows.rs, manual_feed.rs).
    let mut text = if super::human::has_head(&r.text) {
        r.text.clone()
    } else {
        format!("Ручное управление: {}\n{}", r.kind.title(), r.text)
    };
    if !notes.is_empty() {
        text.push_str(&format!("\n\n{}", notes.join("\n")));
    }
    let rows: Vec<Value> = btns.into_iter().map(|b| json!([b])).collect();
    let kb = (!rows.is_empty()).then(|| json!({ "inline_keyboard": rows }));
    (text, kb)
}

/// Напоминание пачкой: список записей (номер, вид, поля) и по ряду кнопок на запись с номером в надписи
/// («1 · Продать сейчас»); одна запись — без номеров. Принимает: записи, заголовок.
/// Возвращает: (текст, клавиатура или None).
pub fn batch_message(recs: &[&ManualRecord], header: &str) -> (String, Option<Value>) {
    batch_message_for(recs, header, false)
}

/// Как batch_message; all — показывать и недоступные кнопки (чат «Неудачные», решение 176). Ошибок нет.
pub fn batch_message_for(
    recs: &[&ManualRecord],
    header: &str,
    all: bool,
) -> (String, Option<Value>) {
    let many = recs.len() > 1;
    let mut text = header.to_string();
    let mut rows = Vec::new();
    for (i, r) in recs.iter().enumerate() {
        let n = i + 1;
        let prefix = if many {
            format!("{n} · ")
        } else {
            String::new()
        };
        let (btns, notes) = buttons_for(r, &prefix, all);
        let num = if many {
            format!("{n}. ")
        } else {
            String::new()
        };
        // Решение 164: у записи свой заголовок — вид записи не повторяем.
        if super::human::has_head(&r.text) {
            text.push_str(&format!("\n\n{num}{}", r.text));
        } else {
            text.push_str(&format!("\n\n{num}{}\n{}", r.kind.title(), r.text));
        }
        if !notes.is_empty() {
            text.push_str(&format!("\n{}", notes.join("\n")));
        }
        if !btns.is_empty() {
            rows.push(json!(btns));
        }
    }
    let kb = (!rows.is_empty()).then(|| json!({ "inline_keyboard": rows }));
    (text, kb)
}

/// Заголовок напоминания. Принимает: сколько записей ждут действия.
pub fn reminder_header(n: usize) -> String {
    format!("⚠️ Напоминание: ждут вашего решения — {n} (экран «Ручное управление»)")
}

/// Текст сообщения записи после того, как она ушла с экрана. Принимает: прежний текст, итог (None — неизвестен).
pub fn closed_text(old: &str, outcome: Option<&str>) -> String {
    match outcome {
        Some(o) => format!("{old}\n\nВыполнено: {o}"),
        None => format!("{old}\n\nГотово: запись ушла с экрана «Ручное управление»."),
    }
}
