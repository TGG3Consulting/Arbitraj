//! Файл: notify/acts.rs
//! Что делает: решение 408 (t193, Тигран 18.11: «обрабатывать тревоги прямо из Telegram») — динамические кнопки у
//! конкретного события (тревоги): какие кнопки положить, решает место события в ядре (там известны площадка, кошелёк,
//! монета, сделка). Кнопка — надпись и callback_data: «c:…» — тот же callback, что у меню /menu (menu_press.rs
//! control_action: та же проверка доступности и «Да/Отмена»), «e:…» — действия тревог (notify/press_event.rs).
//! Клавиатура — по кнопке в ряду; callback_data длиннее 64 байт — кнопки нет, в тексте «только в интерфейсе» (как у
//! записей «Ручного управления», mirror_view.rs). Отправка (acts_send.rs): Notifier::send_acts / send_tagged_acts — задание очереди
//! (notify/queue.rs) с кнопками в task-local (как снимок карточки alert_card.rs); dispatch.rs deliver и
//! dispatch_role.rs to_role берут их оттуда — кнопки и в «Общем», и в чатах ролей.
//! Кто вызывает: core (deps.rs notify_acts / notify_tagged_acts — места тревог), notify/dispatch.rs, dispatch_role.rs.
//! От чего зависит: notify/callback.rs (предел 64 байта), queue.rs, dispatch.rs, dispatch_role.rs, human.rs,
//! core/day_pair.rs (подпись направления).
//! Настройки: нет.
//!
//! InlineKeyboardMarkup / InlineKeyboardButton (callback_data 1–64 байта):
//! https://core.telegram.org/bots/api#inlinekeyboardmarkup, https://core.telegram.org/bots/api#inlinekeyboardbutton

use serde_json::{Value, json};

use super::callback::CALLBACK_DATA_MAX_BYTES;

/// Кнопка события: надпись и callback_data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Act {
    pub title: String,
    pub data: String,
}

tokio::task_local! {
    /// Кнопки задания очереди уведомлений (Notifier::send_acts).
    static ACTS: Vec<Act>;
}

/// Кнопка. Принимает: надпись, callback_data. Ошибок нет.
pub fn act(title: impl Into<String>, data: impl Into<String>) -> Act {
    Act {
        title: title.into(),
        data: data.into(),
    }
}

/// Клавиатура: по кнопке в ряду. Кнопка с пустым или длиннее 64 байт callback_data — не в клавиатуре, а строкой
/// ««надпись» — только в интерфейсе: …». Принимает: кнопки. Возвращает: (клавиатура или None, строки для текста).
/// Ошибок нет.
pub fn keyboard(acts: &[Act]) -> (Option<Value>, Vec<String>) {
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    for a in acts {
        let n = a.data.len();
        if n == 0 || n > CALLBACK_DATA_MAX_BYTES {
            notes.push(format!(
                "«{}» — только в интерфейсе: callback_data {n} байт, Telegram принимает 1–{CALLBACK_DATA_MAX_BYTES}",
                a.title
            ));
            continue;
        }
        rows.push(json!([{ "text": a.title, "callback_data": a.data }]));
    }
    (
        (!rows.is_empty()).then(|| json!({ "inline_keyboard": rows })),
        notes,
    )
}

/// Будущее с кнопками события (берут deliver и to_role). Принимает: кнопки, будущее. Ошибок нет.
pub fn scoped<F: std::future::Future>(
    acts: Vec<Act>,
    f: F,
) -> impl std::future::Future<Output = F::Output> {
    ACTS.scope(acts, f)
}

/// Кнопки текущего задания (пусто — у события их нет). Ошибок нет.
pub fn current() -> Vec<Act> {
    ACTS.try_with(Clone::clone).unwrap_or_default()
}

/// «Снять паузу» площадки или направления (id пары «площадка × тип направления», решение 258) — c:vr. Ошибок нет.
pub fn resume_venue(v: &str) -> Act {
    let t = if crate::core::day_pair::is_pair(v) {
        format!(
            "Снять паузу направления {}",
            crate::core::day_pair::label(v)
        )
    } else {
        format!("Снять паузу {}", super::human::venue(v))
    };
    act(t, format!("c:vr.{v}"))
}

/// «Снять паузу» кошелька DEX (решение 230) — c:wr. Принимает: id кошелька, имя. Ошибок нет.
pub fn resume_wallet(id: &str, name: &str) -> Act {
    act(
        format!("Снять паузу кошелька «{name}»"),
        format!("c:wr.{id}"),
    )
}

/// «Снять карантин» — c:qr. Принимает: id карантина (адрес или тикер), подпись. Ошибок нет.
pub fn release_quarantine(id: &str, label: &str) -> Act {
    act(format!("Снять карантин {label}"), format!("c:qr.{id}"))
}

/// «Сбросить лимит дня» — общий (c:dr.x). Ошибок нет.
pub fn reset_daily() -> Act {
    act("Сбросить лимит дня", "c:dr.x")
}

/// «Старт» — c:mode.Running. Ошибок нет.
pub fn start() -> Act {
    act("Старт", "c:mode.Running")
}

/// «Сбросить лимит дня» площадки или направления (решения 173, 258) — c:dv. Ошибок нет.
pub fn reset_venue_limit(v: &str) -> Act {
    let t = if crate::core::day_pair::is_pair(v) {
        format!(
            "Сбросить лимит дня направления {}",
            crate::core::day_pair::label(v)
        )
    } else {
        format!("Сбросить лимит дня площадки {}", super::human::venue(v))
    };
    act(t, format!("c:dv.{v}"))
}

/// Ключ конца перевода: «площадка» или «площадка/кошелёк» (как core/usdt_transfer_check.rs End::key). Ошибок нет.
pub fn end_key(venue: &str, wallet: Option<&str>) -> String {
    match wallet {
        Some(w) => format!("{venue}/{w}"),
        None => venue.to_string(),
    }
}

/// Действия тревог «e:…» — notify/acts_e.rs.
pub use super::acts_e as e;
