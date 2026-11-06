//! Файл: notify/ctl/mod.rs
//! Что делает: чат «Управление» Telegram (решение 176, роль Control, notify/roles.rs) — главное меню кнопками:
//! «Состояние», «Балансы» (с «Продать»), «Перевести USDT» (диалог по шагам), «Ручное управление»; лайв-обновление
//! экранов и ход перевода. Каждая кнопка — та же команда ядра, что у интерфейса (Core::command / ManualAction), через
//! общий вопрос «Да, …» (notify/press_confirm.rs). Подробности — README.md этой папки.
//! Кто вызывает: notify/press.rs, notify/tg_commands.rs, notify/press_confirm.rs, notify/updates*.rs, main.rs (live).
//! От чего зависит: core, notify (press, press_confirm, menu, roles, client, tg_methods), settings.
//! Настройки: notify.live_refresh_s, notify.live_idle_s, notify.dialog_ttl_s, notify.transfer_pct, notify.control_*.

pub mod balances;
pub mod card;
pub mod kb;
pub mod live;
pub mod press;
pub mod progress;
pub mod screens;
pub mod sell;
pub mod sessions;
pub mod transfer;
pub mod transfer_input;
pub mod transfer_press;
pub mod view;
