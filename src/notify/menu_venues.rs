//! Файл: notify/menu_venues.rs
//! Что делает: экран «Площадки и карантин» меню /menu (решение 157) — паузы площадок, направлений по убытку дня
//! (решение 258: пара «площадка × тип направления», «DEX ↔ DEX» — циклы), кошельков DEX
//! (решение 230) и карантины монет с кнопками снятия — как на плашке режима. Вынесен из notify/menu.rs (ориентир
//! 200 строк); пути menu::venues / menu::venue_buttons сохранены переэкспортом. Площадка подписана человеческим
//! именем (notify/human.rs venue()), как в строке паузы кошелька и остальных экранах Telegram.
//! Кто вызывает: notify/menu.rs (переэкспорт) → notify/menu_press.rs («m:ven»), notify/ctl/screens.rs («Состояние»).
//! От чего зависит: core/risk.rs (Risk), core/day_pair.rs (подпись пары), notify/menu.rs (кнопка, «Назад»), notify/menu_press.rs (строка кошелька,
//! карантин), notify/human.rs (имя площадки).
//! Настройки: нет.

use serde_json::Value;

use crate::core::risk::Risk;

use super::human::venue;
use super::menu::{b, with_back};

/// «Площадки и карантин»: паузы площадок и кошельков DEX (решение 230), карантины монет с кнопками снятия — как на плашке. Принимает: Risk.
pub fn venues(r: &Risk) -> (String, Value) {
    let (t, rows) = venue_buttons(r);
    (t, with_back(rows))
}

/// Текст и кнопки снятия пауз площадок, направлений (пар), кошельков DEX и карантина (решение 176: и «Состояние» чата «Управление»).
/// Принимает: Risk. Возвращает: (текст, кнопки «c:vr.…», «c:wr.…», «c:qr.…»). Ошибок нет.
pub fn venue_buttons(r: &Risk) -> (String, Vec<Value>) {
    let mut t = String::from("Площадки и карантин");
    let mut rows = Vec::new();
    let mut paused: Vec<_> = r.venue_paused.iter().collect();
    paused.sort_by(|a, b| a.0.0.cmp(&b.0.0));
    for (v, why) in paused {
        // Имя площадки — человеческое (notify/human.rs venue()), callback_data — id площадки, как раньше.
        t.push_str(&format!("\nПлощадка {} на паузе: {why}", venue(&v.0)));
        rows.push(b(
            &format!("Снять паузу {}", venue(&v.0)),
            &format!("c:vr.{}", v.0),
        ));
    }
    // Решение 258: пауза одного направления одной биржи по убытку дня (у «DEX ↔ DEX» — только циклы).
    for (p, why) in &r.pair_paused {
        let l = crate::core::day_pair::label(p);
        if crate::core::day_pair::venue_of(p).is_none() {
            t.push_str(&format!("\nЦиклы DEX ↔ DEX на паузе: {why}"));
            rows.push(b("Снять паузу циклов DEX ↔ DEX", &format!("c:vr.{p}")));
        } else {
            t.push_str(&format!("\nНаправление {l} на паузе: {why}"));
            rows.push(b(&format!("Снять паузу {l}"), &format!("c:vr.{p}")));
        }
    }
    // Решение 230: пауза кошелька DEX от сверки — та же команда ядра, что «Снять паузу» на плашке (resume_wallet).
    for (id, p) in &r.wallet_paused {
        t.push_str(&format!("\n{}", super::menu_press::wallet_line(p)));
        rows.push(b(
            &format!("Снять паузу кошелька «{}»", p.name),
            &format!("c:wr.{id}"),
        ));
    }
    for q in super::menu_press::quarantine(r) {
        t.push_str(&format!("\nМонета {} в карантине: {}", q.label, q.reason));
        rows.push(b(
            &format!("Снять карантин {}", q.label),
            &format!("c:qr.{}", q.id),
        ));
    }
    if rows.is_empty() {
        t.push_str("\nПлощадок и кошельков на паузе, монет в карантине нет.");
    }
    (t, rows)
}
