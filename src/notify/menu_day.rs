//! Файл: notify/menu_day.rs
//! Что делает: экран «Лимит дня» меню Telegram и строки лимита для «Состояния» (решения 157, 173; вынесено из
//! menu.rs): итог дня всей системы и общий лимит (risk.max_daily_loss_total_usd), итог дня по парам «площадка × тип
//! направления» (решение 258) — факт, оценка, лимит пары, пауза; кнопки «Сбросить лимит дня» общий (c:dr.x) и по паре
//! (c:dv.<id пары>, core/day_pair.rs).
//! Те же данные, что плашка режима (GET /api/control → Risk.day).
//! Кто вызывает: notify/menu.rs (state), notify/menu_press.rs (screen «day»).
//! От чего зависит: core/risk.rs (Risk), core/day_view.rs (LimitView, PairDay — подпись пары).
//! Настройки: нет.

use serde_json::{Value, json};

use crate::core::day_view::LimitView;
use crate::core::risk::Risk;

/// Кнопка. Принимает: надпись, callback_data.
fn b(text: &str, data: &str) -> Value {
    json!({ "text": text, "callback_data": data })
}

/// «X из Y USDT — достигнут». Принимает: лимит.
fn of(l: &LimitView) -> String {
    format!(
        "{} из {} USDT{}",
        l.loss_usd,
        l.max_usd,
        if l.exceeded {
            " — достигнут"
        } else {
            ""
        }
    )
}

/// Строка общего лимита дня — как на плашке. Принимает: Risk.
pub fn limit_line(r: &Risk) -> String {
    match &r.day.limit {
        Some(l) => format!(
            "Общий лимит дня (Стоп всей системы): {}{}",
            of(l),
            r.day
                .limit_reset
                .as_ref()
                .map(|x| format!(" (сброшен {} — {})", x.at.format("%H:%M:%S UTC"), x.by))
                .unwrap_or_default()
        ),
        None => {
            "Общий лимит дня не задан (risk.max_daily_loss_total_usd) — общего стопа по убытку нет"
                .into()
        }
    }
}

/// Строки пар: «DEX → MEXC: итог −20 USDT (факт −5, оценка −15), лимит 20 из 15 USDT — достигнут, на паузе».
/// Пары без сделок за день и без лимита не показываются. Принимает: Risk.
pub fn venue_lines(r: &Risk) -> Vec<String> {
    r.day
        .pairs
        .iter()
        .filter(|(_, x)| !x.result_usd.is_zero() || x.limit.is_some() || x.paused)
        .map(|(_, x)| {
            let lim = x
                .limit
                .as_ref()
                .map(|l| format!(", лимит {}", of(l)))
                .unwrap_or_else(|| ", лимит не задан".into());
            format!(
                "{}: итог {} USDT (факт {}, оценка {}){lim}{}",
                x.label,
                x.result_usd,
                x.final_usd,
                x.provisional_usd,
                if x.paused { ", на паузе" } else { "" }
            )
        })
        .collect()
}

/// «Лимит дня»: итог дня, общий лимит, пары и кнопки сброса (общий и по паре). Принимает: Risk.
/// Возвращает: (текст, кнопки — по одной в ряду; «Назад» добавляет menu.rs).
pub fn day(r: &Risk) -> (String, Vec<Value>) {
    let d = &r.day;
    let mut t = format!(
        "Итог дня {}: {} USDT (факт {}, оценка позиций {})\n{}",
        d.date,
        d.result_usd,
        d.final_usd,
        d.provisional_usd,
        limit_line(r)
    );
    let mut rows = vec![b("Сбросить лимит дня (все)", "c:dr.x")];
    for line in venue_lines(r) {
        t.push('\n');
        t.push_str(&line);
    }
    for (p, x) in &d.pairs {
        if x.limit.is_some() || x.paused {
            rows.push(b(
                &format!("Сбросить лимит дня {}", x.label),
                &format!("c:dv.{p}"),
            ));
        }
    }
    (t, rows)
}
