//! Файл: core/delist_text.rs
//! Что делает: тексты делистингов для человека (решение 175; вид — решение 164, notify/human): время «05.11 00:00 UTC»,
//! остаток «3 ч», строка пары «ION (ION-USDT): торги до 05.11 00:00 UTC, вывод до 07.12 08:00 UTC» и сообщение
//! уведомления venue.delisting.
//! Кто вызывает: core/delist.rs (уведомление), core/delist_check.rs (причина отказа).
//! От чего зависит: chrono, notify/human.rs.
//! Настройки: нет — это формат отображения.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::DelistItem;
use crate::notify::human::{self, Msg, WARN};

/// Время для человека: «05.11 00:00 UTC». Ошибок нет.
pub fn when(t: DateTime<Utc>) -> String {
    t.format("%d.%m %H:%M UTC").to_string()
}

/// Остаток в часах для человека: «3 ч», «0.5 ч» (один знак, без «.0»). Принимает: секунды. Ошибок нет.
pub fn hours(secs: i64) -> String {
    let h = (Decimal::from(secs) / Decimal::from(3600))
        .round_dp(1)
        .normalize();
    format!("{h} ч")
}

/// Строка пары: «ION (ION-USDT): торги до 05.11 00:00 UTC, вывод до 07.12 08:00 UTC». Время не сообщено — так и
/// пишется. Ошибок нет.
pub fn item(i: &DelistItem) -> String {
    let trade = match i.trading_end {
        Some(t) => format!("торги до {}", when(t)),
        None if i.st => "метка ST (блок — при delist.st_block = да)".into(),
        None => "время прекращения торгов не сообщено".into(),
    };
    let wd = i
        .withdraw_end
        .map(|t| format!(", вывод до {}", when(t)))
        .unwrap_or_default();
    format!("{} ({}): {trade}{wd}", i.ticker, i.symbol)
}

/// Сообщение venue.delisting. Принимает: площадку, новые пары, первая ли это загрузка. Возвращает: текст. Ошибок нет.
pub fn message(venue: &str, items: &[&DelistItem], first: bool) -> String {
    let head = if first {
        format!(
            "{}: пары в делистинге ({})",
            human::venue(venue),
            items.len()
        )
    } else {
        format!("{}: новая пара в делистинге", human::venue(venue))
    };
    let mut m = Msg::new(WARN, head);
    for i in items {
        m = m.line(item(i));
    }
    let src = items.first().map(|i| i.source.clone()).unwrap_or_default();
    m.kv("Источник", src)
        .todo("Сделки по монете на этой бирже не начинаются, если до делистинга меньше delist.block_hours")
        .render()
}
