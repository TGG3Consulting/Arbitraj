//! Файл: notify/press_confirm.rs
//! Что делает: подтверждение действий из Telegram (решения 156, 157) — как confirm на экране: вопрос с кнопками
//! «Да, …» и «Отмена»; «Да» передаёт команду ядру тем же путём, что и экран (Core::command, источник — Telegram,
//! кто нажал — имя пользователя), и правит сообщение вопроса: «Выполнено: кто, что, итог» или «Отклонено: причина»,
//! кнопки убираются. Второе нажатие того же «Да» или по уже выполненной записи — «Уже выполнено», действие не
//! повторяется. Журнал: ядро пишет команду с источником и пользователем; здесь — кто нажал и итог.
//! Кто вызывает: notify/press.rs, press_record.rs, menu_press.rs, tg_commands.rs.
//! От чего зависит: core (Core::command), core/types/control.rs, mirror_state.rs, tg_methods.rs, storage/journal.
//! Настройки: нет.

use chrono::Utc;
use serde_json::json;

use crate::core::types::{CommandSource, ManualCommand};
use crate::storage::journal::Level;

use super::mirror_state::{Pending, lock};
use super::press::Ctx;
use super::press_record::ALREADY;
use super::tg_buttons::{CallbackQuery, CbMessage, User};

/// Надпись «Да, …»: «Да, продать сейчас». Принимает: надпись кнопки.
pub fn yes_title(title: &str) -> String {
    let mut c = title.chars();
    match c.next() {
        Some(f) => format!("Да, {}{}", f.to_lowercase(), c.as_str()),
        None => "Да".into(),
    }
}

impl Ctx<'_> {
    /// Отправляет вопрос подтверждения. Принимает: id нажатия (ответить; «» — не нажатие), ответ на сообщение,
    /// ожидание. Ошибок нет: сбой — в журнал и нажавшему.
    pub async fn ask(&self, query: &str, reply_to: Option<i64>, p: Pending) {
        let (q, title) = (p.question.clone(), p.title.clone());
        let n = lock(self.state).add_pending(p);
        let kb = json!({ "inline_keyboard": [[
            { "text": yes_title(&title), "callback_data": format!("y:{n}") },
            { "text": "Отмена", "callback_data": format!("n:{n}") },
        ]] });
        let sent = self.tg.send_markup(self.chat, &q, Some(kb), reply_to).await;
        let reply = match &sent {
            Ok(_) => "Подтвердите кнопкой «Да» ниже".to_string(),
            Err(e) => format!("Вопрос подтверждения не отправлен: {e}"),
        };
        if let Err(e) = sent {
            lock(self.state).pending.remove(&n);
            self.log(
                Level::Error,
                format!("«{title}»: вопрос подтверждения не отправлен: {e}"),
            )
            .await;
        }
        if !query.is_empty() {
            self.answer(query, Some(&reply)).await;
        }
    }

    /// «Да» (yes) или «Отмена» по номеру ожидания. Принимает: нажатие, сообщение вопроса, номер. Ошибок нет.
    pub async fn confirm(&self, q: &CallbackQuery, msg: &CbMessage, n: u64, yes: bool) {
        let who = q.from.describe();
        let Some(p) = lock(self.state).pending.remove(&n) else {
            return self.answer(&q.id, Some("Уже выполнено или отменено")).await;
        };
        let base = msg.text.clone().unwrap_or_else(|| p.question.clone());
        let chat = msg.chat.id.to_string();
        if !yes {
            self.answer(&q.id, Some("Отменено")).await;
            let _ = self
                .tg
                .edit_text_markup(
                    &chat,
                    msg.message_id,
                    &format!("{base}\n\nОтменено: {who}."),
                    None,
                )
                .await;
            return self
                .log(
                    Level::Info,
                    format!("Telegram: {who} отменил «{}»", p.title),
                )
                .await;
        }
        if let Some(k) = &p.key
            && self.live_record(k).is_none()
        {
            self.answer(&q.id, Some(ALREADY)).await;
            let _ = self
                .tg
                .edit_text_markup(
                    &chat,
                    msg.message_id,
                    &format!("{base}\n\n{ALREADY}."),
                    None,
                )
                .await;
            return;
        }
        self.answer(&q.id, Some(&format!("Выполняю «{}»…", p.title)))
            .await;
        let text = self.execute(&p, &q.from, msg.chat.id).await;
        if let Err(e) = self
            .tg
            .edit_text_markup(&chat, msg.message_id, &format!("{base}\n\n{text}"), None)
            .await
        {
            self.log(
                Level::Warn,
                format!("Вопрос подтверждения не исправлен: {e}"),
            )
            .await;
        }
    }

    /// Передаёт команду ядру и отмечает запись выполненной. Принимает: ожидание, кто, чат.
    /// Возвращает: строку итога «Выполнено: …» или «Отклонено: …». Ошибок нет.
    pub async fn execute(&self, p: &Pending, from: &User, chat: i64) -> String {
        let who = from.describe();
        let cmd = ManualCommand {
            action: p.action.clone(),
            source: CommandSource::Telegram {
                chat: chat.to_string(),
            },
            by: Some(who.clone()),
            at: Utc::now(),
        };
        // Запись отмечается выполненной ДО команды: второе нажатие во время выполнения — «уже выполнено».
        // Ушла с экрана — отметку снимает mirror_state (запись с тем же ключом может вернуться); отказ — снимаем здесь.
        if let Some(k) = &p.key {
            lock(self.state).done.insert(k.clone());
        }
        let res = self.core.command(cmd).await;
        if res.is_err()
            && let Some(k) = &p.key
        {
            lock(self.state).done.remove(k);
        }
        let at = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
        let (level, text) = match res {
            Ok(t) => (
                Level::Info,
                format!("Выполнено: {who}, «{}» — {t}, {at}.", p.title),
            ),
            Err(e) => (
                Level::Warn,
                format!("Отклонено: {who}, «{}» — {e}, {at}.", p.title),
            ),
        };
        let data = json!({ "action": p.action, "from": who, "chat": chat });
        let _ = self
            .journal
            .write(
                level,
                "notify",
                None,
                format!("Telegram: {text}"),
                Some(data),
            )
            .await;
        text
    }
}
