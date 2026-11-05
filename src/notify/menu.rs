//! Файл: notify/menu.rs
//! Что делает: экраны меню управления в Telegram (решение 157, команды /menu и /dev) — текст и inline-кнопки из
//! тех же данных, что плашка режима сверху интерфейса (ControlBar.svelte, GET /api/control → Risk) и экран
//! «Ручное управление» (записи с кнопками, core/manual_feed.rs). Доступность кнопок — как на плашке:
//! «Старт» — если не «В работе», «Пауза» — только из «В работе», «Стоп» — если не «Стоп». Настроек из Telegram нет.
//! Кто вызывает: notify/menu_press.rs, notify/tg_commands.rs, notify/ctl/screens.rs (решение 176: «Состояние» чата
//! «Управление» — те же кнопки).
//! От чего зависит: core/risk.rs (Risk, Mode), core/commands_control.rs (label), notify/mirror_view.rs,
//! notify/menu_day.rs (лимит дня — общий и по площадкам, решение 173).
//! Настройки: нет.
//!
//! callback_data: «m:<экран>» — переход (root, st, man, mode, ven, day); «c:<что>» — кнопка плашки
//! (mode.Running|Paused|Stopped, vr.<площадка>, qr.<id монеты>, dr.x — общий сброс лимита дня, dv.<площадка> — сброс
//! лимита дня площадки, решение 173) — подтверждение в menu_press.rs.

use serde_json::{Value, json};

use crate::core::commands_control::label;
use crate::core::risk::{Mode, Risk};
use crate::core::types::ManualRecord;

use super::mirror_view::batch_message;

/// Кнопка. Принимает: надпись, callback_data.
fn b(text: &str, data: &str) -> Value {
    json!({ "text": text, "callback_data": data })
}

/// Клавиатура: по кнопке в ряду, внизу «Назад» в главное меню. Принимает: кнопки.
fn with_back(rows: Vec<Value>) -> Value {
    let mut rows: Vec<Value> = rows.into_iter().map(|x| json!([x])).collect();
    rows.push(json!([b("« Назад", "m:root")]));
    json!({ "inline_keyboard": rows })
}

/// Главное меню. Возвращает: (текст, клавиатура).
pub fn root() -> (String, Value) {
    let rows = json!({ "inline_keyboard": [
        [b("Состояние", "m:st")], [b("Ручное управление", "m:man")], [b("Режим системы", "m:mode")],
        [b("Площадки и карантин", "m:ven")], [b("Лимит дня", "m:day")],
    ] });
    (
        "Меню управления ARB. Изменение настроек — только в интерфейсе.".into(),
        rows,
    )
}

/// «Состояние» — всё, что показывает плашка режима. Принимает: Risk, сколько записей ждут на «Ручном управлении».
pub fn state(r: &Risk, manual: usize) -> (String, Value) {
    let mut t = format!("Режим: {}", label(r.mode));
    if let Some(why) = &r.reason {
        t.push_str(&format!(" — {why}"));
    }
    let d = &r.day;
    t.push_str(&format!(
        "\nИтог дня {}: {} USDT (факт {}, оценка позиций {})\n{}",
        d.date,
        d.result_usd,
        d.final_usd,
        d.provisional_usd,
        super::menu_day::limit_line(r)
    ));
    // Решение 173: итог дня и лимит по площадкам.
    for line in super::menu_day::venue_lines(r) {
        t.push_str(&format!("\n{line}"));
    }
    if r.daily_stop && d.limit.as_ref().is_some_and(|l| !l.exceeded) {
        t.push_str("\nЛимит сейчас не превышен — остановка по лимиту сама не снимается, можно нажать «Старт».");
    }
    for (v, why) in &r.venue_paused {
        t.push_str(&format!("\nПлощадка {} на паузе: {why}", v.0));
    }
    for why in r.cycle_paused.values() {
        t.push_str(&format!("\nЦиклы DEX ↔ DEX на паузе: {why}"));
    }
    for (deal, (usd, day)) in &d.provisional {
        let unpriced = if d.unpriced.contains(deal) {
            " — цены нет, взят худший случай (вся сумма)"
        } else {
            ""
        };
        t.push_str(&format!(
            "\nОценка позиции сделки {deal}: {usd} USDT (учтена {day}){unpriced} — ждёт разбора"
        ));
    }
    for q in super::menu_press::quarantine(r) {
        t.push_str(&format!("\nМонета {} в карантине: {}", q.label, q.reason));
    }
    t.push_str(&format!(
        "\nРучное управление: ждут действия записей — {manual}"
    ));
    (t, with_back(vec![b("Обновить", "m:st")]))
}

/// Кнопки режима, доступные сейчас — как на плашке (решение 176: и экран «Состояние» чата «Управление»).
/// Принимает: Risk. Возвращает: кнопки «c:mode.<режим>». Ошибок нет.
pub fn mode_buttons(r: &Risk) -> Vec<Value> {
    let mut rows = Vec::new();
    for (m, title) in [
        (Mode::Running, "Старт"),
        (Mode::Paused, "Пауза"),
        (Mode::Stopped, "Стоп"),
    ] {
        if mode_disabled(r.mode, m).is_none() {
            rows.push(b(title, &format!("c:mode.{m:?}")));
        }
    }
    rows
}

/// «Режим системы»: доступные кнопки — как на плашке. Принимает: Risk.
pub fn mode(r: &Risk) -> (String, Value) {
    let rows = mode_buttons(r);
    (
        format!(
            "Режим системы: {}{}",
            label(r.mode),
            r.reason
                .as_ref()
                .map(|x| format!(" — {x}"))
                .unwrap_or_default()
        ),
        with_back(rows),
    )
}

/// Недоступна ли смена режима — как disabled у кнопок плашки. Принимает: текущий и желаемый режим.
/// Возвращает: причину или None — можно.
pub fn mode_disabled(now: Mode, want: Mode) -> Option<String> {
    let off = match want {
        Mode::Running => now == Mode::Running,
        Mode::Paused => now != Mode::Running,
        Mode::Stopped => now == Mode::Stopped,
    };
    off.then(|| format!("сейчас режим «{}»", label(now)))
}

/// «Площадки и карантин»: паузы площадок и карантины монет с кнопками снятия — как на плашке. Принимает: Risk.
pub fn venues(r: &Risk) -> (String, Value) {
    let (t, rows) = venue_buttons(r);
    (t, with_back(rows))
}

/// Текст и кнопки снятия пауз площадок, циклов и карантина (решение 176: и «Состояние» чата «Управление»).
/// Принимает: Risk. Возвращает: (текст, кнопки «c:vr.…», «c:qr.…»). Ошибок нет.
pub fn venue_buttons(r: &Risk) -> (String, Vec<Value>) {
    let mut t = String::from("Площадки и карантин");
    let mut rows = Vec::new();
    let mut paused: Vec<_> = r.venue_paused.iter().collect();
    paused.sort_by(|a, b| a.0.0.cmp(&b.0.0));
    for (v, why) in paused {
        t.push_str(&format!("\nПлощадка {} на паузе: {why}", v.0));
        rows.push(b(&format!("Снять паузу {}", v.0), &format!("c:vr.{}", v.0)));
    }
    // Решение 173: пауза только циклов DEX ↔ DEX по убытку дня DEX.
    for (v, why) in &r.cycle_paused {
        t.push_str(&format!("\nЦиклы DEX ↔ DEX на паузе: {why}"));
        rows.push(b("Снять паузу циклов DEX ↔ DEX", &format!("c:vr.{v}")));
    }
    for q in super::menu_press::quarantine(r) {
        t.push_str(&format!("\nМонета {} в карантине: {}", q.label, q.reason));
        rows.push(b(
            &format!("Снять карантин {}", q.label),
            &format!("c:qr.{}", q.id),
        ));
    }
    if rows.is_empty() {
        t.push_str("\nПлощадок на паузе и монет в карантине нет.");
    }
    (t, rows)
}

/// «Лимит дня»: итог дня, общий лимит и лимиты площадок, кнопки «Сбросить лимит дня» — общий и по площадке
/// (решения 153, 173; notify/menu_day.rs). Принимает: Risk.
pub fn day(r: &Risk) -> (String, Value) {
    let (t, rows) = super::menu_day::day(r);
    (t, with_back(rows))
}

/// «Ручное управление»: ждущие записи с их кнопками (как напоминание) и «Назад». Принимает: записи.
pub fn manual(recs: &[&ManualRecord]) -> (String, Value) {
    if recs.is_empty() {
        return (
            "Ручное управление: записей, ждущих действия, нет.".into(),
            with_back(vec![b("Обновить", "m:man")]),
        );
    }
    let (t, kb) = batch_message(
        recs,
        &format!("Ручное управление: ждут действия записей — {}", recs.len()),
    );
    let mut rows = kb
        .and_then(|k| k["inline_keyboard"].as_array().cloned())
        .unwrap_or_default();
    rows.push(json!([b("Обновить", "m:man")]));
    rows.push(json!([b("« Назад", "m:root")]));
    (t, json!({ "inline_keyboard": rows }))
}
