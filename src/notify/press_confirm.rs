//! Файл: notify/press_confirm.rs
//! Что делает: подтверждение действий из Telegram (решения 156, 157) — как confirm на экране: вопрос с кнопками
//! «Да, …» и «Отмена»; «Да» передаёт команду ядру тем же путём, что и экран (Core::command, источник — Telegram,
//! кто нажал — имя пользователя), и правит сообщение вопроса: «Выполнено: кто, что, итог» или «Отклонено: причина»,
//! кнопки убираются. Второе нажатие того же «Да» или по уже выполненной записи — «Уже выполнено», действие не
//! повторяется. Журнал: ядро пишет команду с источником и пользователем; здесь — кто нажал и итог.
//! Решение 176: «Да» перевода USDT — отдельной задачей с ходом перевода в том же сообщении (notify/ctl/progress.rs).
//! Решение 408 (t193): команда ушла ждать окончания сделок монеты (решение 405) — «Ждёт: …», не «Выполнено»; сообщение
//! запоминается (mirror_wait.rs: конец ожидания без выполнения — «Не выполнено: причина», кнопки снова доступны).
//! Кто вызывает: notify/press.rs, press_record.rs, menu_press.rs, tg_commands.rs, notify/ctl/*.
//! От чего зависит: core (Core::command), core/types/control.rs, mirror_state.rs, tg_methods.rs, storage/journal.
//! Настройки: нет.

use serde_json::json;

use crate::core::types::ManualAction;
use crate::storage::journal::Level;

use super::mirror_state::{Pending, lock};
use super::press::Ctx;
use super::press_record::ALREADY;
pub use super::press_run::{run_pending, run_pending_full};
use super::tg_buttons::{CallbackQuery, CbMessage};

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
        let n = {
            let mut st = lock(self.state);
            // Ревью t193 (Н1): вопросы без ответа старше notify.dialog_ttl_s — убираются (не задана — как раньше).
            if let Ok(ttl) = self.ttl() {
                st.purge_pending(ttl);
            }
            st.add_pending(p)
        };
        let kb = json!({ "inline_keyboard": [[
            { "text": yes_title(&title), "callback_data": format!("y:{n}") },
            { "text": "Отмена", "callback_data": format!("n:{n}") },
        ]] });
        let sent = self.tg.send_markup(self.chat, &q, Some(kb), reply_to).await;
        let reply = match &sent {
            Ok(_) => "Подтвердите кнопкой «Да» ниже".to_string(),
            Err(e) => format!("Вопрос подтверждения не отправлен: {e}"),
        };
        match sent {
            // Ревью t193 (Н5): «Да» — только с этого сообщения этого чата.
            Ok(id) => {
                lock(self.state)
                    .pending_at
                    .insert(n, (self.chat.to_string(), id));
            }
            Err(e) => {
                {
                    let mut st = lock(self.state);
                    st.pending.remove(&n);
                    st.pending_born.remove(&n);
                }
                self.log(
                    Level::Error,
                    format!("«{title}»: вопрос подтверждения не отправлен: {e}"),
                )
                .await;
            }
        }
        if !query.is_empty() {
            self.answer(query, Some(&reply)).await;
        }
    }

    /// «Да» (yes) или «Отмена» по номеру ожидания. Принимает: нажатие, сообщение вопроса, номер. Ошибок нет.
    pub async fn confirm(&self, q: &CallbackQuery, msg: &CbMessage, n: u64, yes: bool) {
        let who = q.from.describe();
        // Ревью t193 (Н5): номер «Да» привязан к сообщению вопроса и чату — нажатие с другого сообщения не выполняет.
        let own = {
            let mut st = lock(self.state);
            let ok = st.pending_at.get(&n).is_some_and(|(c, m)| {
                *m == msg.message_id && super::roles::chat_matches(c, &msg.chat)
            });
            if ok {
                st.pending_at.remove(&n);
                st.pending_born.remove(&n);
                st.pending.remove(&n)
            } else {
                None
            }
        };
        let Some(p) = own else {
            return self.answer(&q.id, Some("Уже выполнено или отменено")).await;
        };
        let base = msg.text.clone().unwrap_or_else(|| p.question.clone());
        let chat = msg.chat.id.to_string();
        // Ревью t193 (Н1): «Да» денежной команды — не старше notify.dialog_ttl_s от вопроса (не продать через часы всё
        // свободное на тот момент).
        if yes
            && super::press_event_ttl::money_action(&p.action)
            && let Err(why) = self.fresh_at(msg.date)
        {
            self.answer(&q.id, Some(&why)).await;
            let _ = self
                .tg
                .edit_text_markup(
                    &chat,
                    msg.message_id,
                    &format!("{base}\n\nНе выполнено: {why}."),
                    None,
                )
                .await;
            return;
        }
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
        // Решение 176: перевод USDT идёт минуты — отдельной задачей с ходом в том же сообщении (ctl/progress.rs),
        // опрос Telegram не ждёт.
        if let ManualAction::TransferUsdt {
            from, from_wallet, ..
        } = &p.action
        {
            let key = match from_wallet {
                Some(w) => format!("{from}/{w}"),
                None => from.clone(),
            };
            return super::ctl::progress::spawn(
                self,
                p,
                q.from.clone(),
                (msg.chat.id, msg.message_id),
                base,
                key,
            )
            .await;
        }
        let (text, wait) = run_pending_full(
            self.core,
            self.journal,
            self.state,
            &p,
            &q.from,
            msg.chat.id,
        )
        .await;
        // Решение 408: команда ждёт окончания сделок — сообщение запоминается: конец ожидания без выполнения правит его
        // «Не выполнено: причина», кнопки записи снова доступны (notify/mirror_wait.rs).
        // Ревью t193 (М7): конец ожидания мог прийти раньше — тогда он уже в wait_ended и применяется здесь (под одним
        // замком с записью WaitMark — окна нет).
        let mut shown = format!("{base}\n\n{text}");
        if let Some(id) = wait {
            let ended = {
                let mut st = lock(self.state);
                match st.wait_ended.remove(&id) {
                    Some(x) => Some(x),
                    None => {
                        st.waits.insert(
                            id,
                            super::mirror_wait::WaitMark {
                                key: p.key.clone(),
                                role: self.role,
                                chat: chat.clone(),
                                msg: msg.message_id,
                                text: shown.clone(),
                            },
                        );
                        None
                    }
                }
            };
            if let Some((free, line, _)) = ended {
                if free && let Some(k) = &p.key {
                    lock(self.state).done.remove(k);
                }
                shown = format!("{shown}\n\n{line}");
            }
        }
        if let Err(e) = self
            .tg
            .edit_text_markup(&chat, msg.message_id, &shown, None)
            .await
        {
            self.log(
                Level::Warn,
                format!("Вопрос подтверждения не исправлен: {e}"),
            )
            .await;
        }
        // Ревью t193 (М8): «Снять очередь» отклонено, а nonce к этому моменту не занят — сразу второе подтверждение
        // кнопкой (как «Подтверждаю: снять при незанятом nonce» окна интерфейса).
        if let ManualAction::ReleaseWalletQueue {
            venue,
            wallet,
            pending_ok: false,
            ..
        } = &p.action
            && text.starts_with("Отклонено")
        {
            let arg = format!("{venue}.{}", wallet.as_deref().unwrap_or("-"));
            if let Ok(again) = self.ev_queue(&arg).await
                && matches!(
                    again.action,
                    ManualAction::ReleaseWalletQueue {
                        pending_ok: true,
                        ..
                    }
                )
            {
                self.ask("", Some(msg.message_id), again).await;
            }
        }
    }
}
