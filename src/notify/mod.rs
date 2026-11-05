//! Файл: notify/mod.rs
//! Что делает: уведомления в Telegram — какие события слать, решает настройка каждого события.
//! Кто вызывает: main.rs (в т.ч. приём нажатий кнопок и команд — updates.rs, дубль «Ручного управления» — mirror.rs), api, ядро.
//! От чего зависит: settings, secrets, storage/journal. Никогда — venues.
//! Настройки: notify.* (docs/SETTINGS.md).

pub mod callback;
pub mod catalog;
pub mod check;
pub mod client;
pub mod dispatch;
pub mod heads;
pub mod human;
pub mod human_msg;
pub mod menu;
pub mod menu_press;
pub mod mirror;
pub mod mirror_send;
pub mod mirror_state;
pub mod mirror_view;
pub mod press;
pub mod press_confirm;
pub mod press_record;
pub mod reason;
pub mod reason_codes;
pub mod settings_watch;
pub mod telegram;
pub mod tg_buttons;
pub mod tg_commands;
pub mod tg_methods;
pub mod tg_text;
pub mod updates;
pub mod updates_idle;
