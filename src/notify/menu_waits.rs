//! Файл: notify/menu_waits.rs
//! Что делает: решение 408 (t193) — ожидания ручных действий (решение 405: кнопка ждёт окончания идущих сделок монеты)
//! в «Ручном управлении» Telegram: меню /menu «Общего» (menu.rs manual) и экран чата «Управление» (ctl/screens.rs
//! manual) — строка на ожидание (что, монета, кто нажал, с какого момента, чьих окончаний ждём) и кнопка «Отменить
//! ожидание» (e:wc.<ID>, notify/press_event.rs → «Да/Отмена» → команда ядра CancelManualWait, как кнопка экрана).
//! Кто вызывает: notify/menu.rs, notify/ctl/screens.rs.
//! От чего зависит: core/manual_wait.rs (ManualWait), notify/acts.rs (кнопка и предел 64 байта).
//! Настройки: нет.

use serde_json::{Value, json};

use crate::core::manual_wait::ManualWait;

/// Строка ожидания. Принимает: ожидание. Ошибок нет.
pub fn line(w: &ManualWait) -> String {
    format!(
        "⏳ «{}» по {} (нажал: {}, с {}) ждёт окончания: {}",
        w.what,
        w.coin,
        w.who,
        w.since.format("%Y-%m-%d %H:%M:%S UTC"),
        w.waits_for.join("; ")
    )
}

/// Текст (с пустой строкой перед ним) и ряды кнопок «Отменить ожидание». Пусто — ("", []). Принимает: ожидания.
/// Ошибок нет: кнопка длиннее 64 байт — строкой «только в интерфейсе» (acts::keyboard).
pub fn block(waits: &[ManualWait]) -> (String, Vec<Value>) {
    if waits.is_empty() {
        return (String::new(), Vec::new());
    }
    let mut t = format!("\n\nЖдут окончания сделок монеты — {}:", waits.len());
    for w in waits {
        t.push_str(&format!("\n{}", line(w)));
    }
    let acts: Vec<_> = waits
        .iter()
        .map(|w| super::acts::e::cancel_wait(&w.id, &w.what, &w.coin))
        .collect();
    let (kb, notes) = super::acts::keyboard(&acts);
    for n in notes {
        t.push_str(&format!("\n{n}"));
    }
    let rows = kb
        .and_then(|k| k["inline_keyboard"].as_array().cloned())
        .unwrap_or_default();
    (t, rows)
}

/// Ряды кнопок как JSON-массив (для ctl/kb.rs Screen). Ошибок нет.
pub fn rows(waits: &[ManualWait]) -> Vec<Vec<Value>> {
    block(waits)
        .1
        .into_iter()
        .map(|r| r.as_array().cloned().unwrap_or_else(|| vec![json!(r)]))
        .collect()
}
