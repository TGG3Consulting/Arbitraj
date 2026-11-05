//! Файл: notify/menu_day.rs
//! Что делает: экран «Лимит дня» меню Telegram и строки лимита для «Состояния» (решения 157, 173; вынесено из
//! menu.rs): итог дня всей системы и общий лимит (risk.max_daily_loss_total_usd), итог дня по площадкам — факт,
//! оценка, лимит площадки, пауза; кнопки «Сбросить лимит дня» общий (c:dr.x) и по площадке (c:dv.<площадка>).
//! Те же данные, что плашка режима (GET /api/control → Risk.day).
//! Кто вызывает: notify/menu.rs (state), notify/menu_press.rs (screen «day»).
//! От чего зависит: core/risk.rs (Risk), core/day_view.rs (LimitView), notify/human.rs (имя площадки).
//! Настройки: нет.

use serde_json::{Value, json};

use crate::core::day_view::LimitView;
use crate::core::risk::Risk;

use super::human::venue as name;

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

/// Строки площадок: «MEXC: итог −20 USDT (факт −5, оценка −15), лимит 20 из 15 USDT — достигнут, на паузе».
/// Площадки без сделок за день и без лимита не показываются. Принимает: Risk.
pub fn venue_lines(r: &Risk) -> Vec<String> {
    r.day
        .venues
        .iter()
        .filter(|(_, x)| !x.result_usd.is_zero() || x.limit.is_some() || x.paused)
        .map(|(v, x)| {
            let lim = x
                .limit
                .as_ref()
                .map(|l| format!(", лимит {}", of(l)))
                .unwrap_or_else(|| ", лимит не задан".into());
            format!(
                "{}: итог {} USDT (факт {}, оценка {}){lim}{}",
                name(v),
                x.result_usd,
                x.final_usd,
                x.provisional_usd,
                if x.paused { ", на паузе" } else { "" }
            )
        })
        .collect()
}

/// «Лимит дня»: итог дня, общий лимит, площадки и кнопки сброса (общий и по площадке). Принимает: Risk.
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
    for (v, x) in &d.venues {
        if x.limit.is_some() || x.paused {
            rows.push(b(
                &format!("Сбросить лимит дня {}", name(v)),
                &format!("c:dv.{v}"),
            ));
        }
    }
    (t, rows)
}
