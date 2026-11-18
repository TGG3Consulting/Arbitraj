//! Файл: notify/press.rs
//! Что делает: одно нажатие кнопки Telegram (CallbackQuery). Роль чата уже определил опрос (updates_poll.rs, решение
//! 176); здесь — разрешён ли пользователь в этой роли (notify/roles.rs::allowed) и принимает ли роль эту кнопку
//! (accepts: кнопка чужого чата — отказ и журнал), затем разбор callback_data и обработчик:
//! «<код>:<ключ записи>» — кнопка записи «Ручного управления» (press_record.rs, решение 156; чаты «Общий» и «Неудачные»);
//! «y:<n>» / «n:<n>» — «Да, …» / «Отмена» подтверждения (press_confirm.rs); «m:<экран>» — экран меню /menu «Общего»
//! (menu_press.rs, решение 157); «c:<что>» — кнопка плашки режима (menu_press.rs; «Общий» и «Управление»);
//! «k:<экран>», «s:<номер>.<строка>», «t:<номер>.<шаг>» — экраны, «Продать» и перевод USDT чата «Управление»
//! (notify/ctl/*); «r:<что>» — «📊 Отчёты» чата «Управление» (notify/ctl/report.rs, решение 215); «e:<что>» — кнопки
//! тревог (решение 408, notify/press_event.rs; «Общий», «Управление», «Неудачные», «Аккаунты»). На каждое нажатие —
//! answerCallbackQuery.
//! Кто вызывает: notify/updates_poll.rs — на каждое Update с callback_query.
//! От чего зависит: notify/callback.rs (decode), roles.rs, press_record.rs, press_confirm.rs, menu_press.rs, ctl/*, core.
//! Настройки: разрешённые пользователи ролей (notify.control_users, notify.general_users, notify.failures_users,
//! notify.accounts_users — решение 408).

use crate::core::Core;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};

use super::callback;
use super::ctl::sessions::Shared as CtlShared;
use super::mirror_state::Shared;
use super::roles::{self, Role};
use super::telegram::Telegram;
use super::tg_buttons::CallbackQuery;

pub use super::roles::chat_matches;

/// Всё, что нужно обработчикам нажатий и сообщений одного обновления.
pub struct Ctx<'a> {
    pub tg: &'a Telegram,
    /// ID чата роли из настройки: числовой id чата или @username.
    pub chat: &'a str,
    /// Роль чата обновления.
    pub role: Role,
    /// Имя бота (getMe) — для команд вида /cmd@имя_бота в группах.
    pub bot: &'a str,
    pub core: &'a Core,
    pub journal: &'a Journal,
    pub settings: &'a Store,
    /// Память дубля «Ручного управления» (общая с notify/mirror.rs).
    pub state: &'a Shared,
    /// Память экранов чата «Управление» (notify/ctl/sessions.rs).
    pub ctl: &'a CtlShared,
}

/// Принимает ли роль кнопку с этим кодом (кнопка чужого чата — отказ). Принимает: роль, код из callback_data.
/// Возвращает: true — обработать. Ошибок нет.
pub fn accepts(role: Role, code: &str) -> bool {
    match role {
        Role::Deals => false,
        // Решение 408 (Тигран 18.11 «да»): в «Аккаунтах» — «Снять паузу» биржи на его тревогах («c:vr», accepts_data)
        // и «Да/Отмена».
        Role::Accounts => matches!(code, "y" | "n" | "c"),
        // Решение 192: и кнопки записей «Ручного управления» — с карточки проблемной сделки (ctl/card.rs).
        Role::Control => code != "m",
        // Решение 215: «r» — отчёты, только чат «Управление».
        Role::General => !matches!(code, "k" | "s" | "t" | "r"),
        // Решение 408 (Тигран 18.11 «да»): кнопки плашки «c:» и тревог «e:» — и в «Неудачных».
        Role::Failures => !matches!(code, "m" | "k" | "s" | "t" | "r"),
    }
}

/// Ревью t193 (Н4): принимает ли роль кнопку с этими данными — только если такая кнопка могла быть в этом чате:
/// «Аккаунты» — «c:vr.…» (venue.withdraw_limit); «Неудачные» — «c:vr.…» / «c:wr.…» (risk.reconcile_paused) и «e:od.…»
/// (кнопка записи «своих монет не видно»); «Управление» — «e:wc.…» (ожидания на «Ручном управлении»), «e:od.…»
/// (карточка); «Общий» — все кнопки тревог и меню. Принимает: роль, код, объект callback_data. Ошибок нет.
pub fn accepts_data(role: Role, code: &str, object: &str) -> bool {
    if !accepts(role, code) {
        return false;
    }
    match (role, code) {
        (Role::Accounts, "c") => object.starts_with("vr."),
        (Role::Failures, "c") => object.starts_with("vr.") || object.starts_with("wr."),
        (Role::Failures, "e") => object.starts_with("od."),
        (Role::Control, "e") => object.starts_with("wc.") || object.starts_with("od."),
        _ => true,
    }
}

impl Ctx<'_> {
    /// Обрабатывает нажатие. Ошибок не возвращает: всё пишется в журнал, нажавший получает ответ.
    pub async fn handle(&self, q: CallbackQuery) {
        let who = q.from.describe();
        let Some(msg) = q.message.as_ref() else {
            self.answer(&q.id, None).await;
            return self
                .log(
                    Level::Warn,
                    format!("Нажатие без сообщения бота (inline-режим) от {who} — игнор"),
                )
                .await;
        };
        if !roles::allowed(self.settings, self.role, &q.from) {
            self.answer(
                &q.id,
                Some("Нет доступа: вашего пользователя нет в списке разрешённых"),
            )
            .await;
            return self
                .log(
                    Level::Warn,
                    format!(
                        "Чат «{}»: нажатие от неразрешённого {who} — отказ",
                        self.role.title()
                    ),
                )
                .await;
        }
        let Some((code, object)) = q.data.as_deref().and_then(callback::decode) else {
            self.answer(&q.id, Some("Неизвестная кнопка")).await;
            return self
                .log(
                    Level::Warn,
                    format!("Неизвестная кнопка {:?} от {who}", q.data),
                )
                .await;
        };
        if !accepts_data(self.role, code, object) {
            self.answer(&q.id, Some("Эта кнопка не для этого чата"))
                .await;
            return self
                .log(
                    Level::Warn,
                    format!(
                        "Чат «{}»: чужая кнопка {code} от {who} — отказ",
                        self.role.title()
                    ),
                )
                .await;
        }
        match code {
            "y" | "n" => match object.parse::<u64>() {
                Ok(n) => self.confirm(&q, msg, n, code == "y").await,
                Err(_) => self.answer(&q.id, Some("Неизвестная кнопка")).await,
            },
            "m" => self.menu_screen(&q, msg, object).await,
            "c" => self.menu_control(&q, msg, object).await,
            "k" => self.ctl_nav(&q, msg, object).await,
            "s" => self.ctl_sell(&q, msg, object).await,
            "t" => self.ctl_transfer(&q, msg, object).await,
            "r" => self.ctl_report(&q, msg, object).await,
            // Решение 408: действия кнопок тревог (notify/press_event.rs).
            "e" => self.event_button(&q, msg, object).await,
            _ => self.record_button(&q, msg, code, object).await,
        }
    }

    /// answerCallbackQuery; ошибка — в журнал. Принимает: id нажатия, текст ответа.
    pub async fn answer(&self, id: &str, text: Option<&str>) {
        if let Err(e) = self.tg.answer_callback(id, text).await {
            self.log(Level::Warn, format!("answerCallbackQuery: {e}"))
                .await;
        }
    }

    /// Запись в журнал от имени уведомлений. Ошибка журнала уже выведена в лог процесса.
    pub async fn log(&self, level: Level, message: String) {
        let _ = self
            .journal
            .write(level, "notify", None, message, None)
            .await;
    }
}
