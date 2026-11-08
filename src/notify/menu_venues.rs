//! Файл: notify/menu_venues.rs
//! Что делает: экран «Площадки и карантин» меню /menu (решение 157) — паузы площадок, циклов DEX ↔ DEX, кошельков DEX
//! (решение 230) и карантины монет с кнопками снятия — как на плашке режима. Вынесен из notify/menu.rs (ориентир
//! 200 строк); пути menu::venues / menu::venue_buttons сохранены переэкспортом. Площадка подписана человеческим
//! именем (notify/human.rs venue()), как в строке паузы кошелька и остальных экранах Telegram.
//! Кто вызывает: notify/menu.rs (переэкспорт) → notify/menu_press.rs («m:ven»), notify/ctl/screens.rs («Состояние»).
//! От чего зависит: core/risk.rs (Risk), notify/menu.rs (кнопка, «Назад»), notify/menu_press.rs (строка кошелька,
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

/// Текст и кнопки снятия пауз площадок, циклов, кошельков DEX и карантина (решение 176: и «Состояние» чата «Управление»).
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
    // Решение 173: пауза только циклов DEX ↔ DEX по убытку дня DEX.
    for (v, why) in &r.cycle_paused {
        t.push_str(&format!("\nЦиклы DEX ↔ DEX на паузе: {why}"));
        rows.push(b("Снять паузу циклов DEX ↔ DEX", &format!("c:vr.{v}")));
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
