//! Файл: notify/mirror_wait.rs
//! Что делает: решение 408 (t193, М4 ревью t191) — ручная кнопка из Telegram, ушедшая ждать окончания сделок монеты
//! (решение 405, core/manual_wait.rs): итог «Да, …» — «Ждёт: …» (press_confirm.rs), сообщение вопроса запоминается
//! (WaitMark). Дальше по событиям ядра Command:
//! - {final: true, ok: false, wait: ID} — ожидание кончилось без действия (отменено, держащая сделка застряла,
//!   core/manual_wait_run.rs end): отметка «выполнено из Telegram» записи снимается — её кнопки снова доступны (и в
//!   напоминании), сообщение вопроса правится «Не выполнено: причина»;
//! - {ok: true, wait: ID} — сама команда ответила «ждёт»: записи не «выполнены» (сообщения не правятся «Выполнено»);
//! - {retry: ID} — итог повтора команды после окончания сделок: ok — «Выполнено: …» в сообщении вопроса (запись ушла с
//!   экрана — её сообщения правит обычный итог, mirror_send.rs); отказ — как конец без выполнения.
//!
//! Ожидание начато не из Telegram (экран, «Балансы») — сообщения нет, ничего не правится.
//! Кто вызывает: notify/mirror_send.rs (outcome), notify/press_confirm.rs (WaitMark).
//! От чего зависит: mirror.rs (Mirror), mirror_state.rs, client.rs (бот чата роли), tg_methods.rs, roles.rs.
//! Настройки: нет (бот и чат — роли нажатия).

use crate::storage::journal::Level;

use super::client;
use super::mirror::Mirror;
use super::mirror_state::lock;
use super::roles::Role;

/// Сообщение вопроса «Да, …» команды, которая ждёт окончания сделок монеты.
#[derive(Debug, Clone)]
pub struct WaitMark {
    /// Ключ записи «Ручного управления» (None — кнопка не записи).
    pub key: Option<String>,
    /// Роль чата нажатия (его бот правит сообщение).
    pub role: Role,
    pub chat: String,
    pub msg: i64,
    /// Текст сообщения сейчас («вопрос + Ждёт: …»).
    pub text: String,
}

impl Mirror {
    /// Итог команды с ожиданием. Принимает: событие Command. Возвращает: true — обработано полностью (обычный итог не
    /// нужен). Ошибок нет: сбой правки — в журнал.
    pub(super) async fn wait_outcome(&self, v: &serde_json::Value) -> bool {
        let text = v["text"].as_str().unwrap_or_default();
        let ok = v["ok"].as_bool() == Some(true);
        // Ревью t193 (М7): ожидание начато из Telegram — его «Да» может ещё не записать WaitMark.
        let tg = v["who"].as_str().is_some_and(|w| w.starts_with("Telegram"));
        if let Some(id) = v["retry"].as_str() {
            let line = if ok {
                format!("Выполнено после окончания сделок: {text}")
            } else {
                format!("Не выполнено: {text}")
            };
            self.wait_end(id, !ok, &line, tg).await;
            return false;
        }
        let Some(id) = v["wait"].as_str() else {
            return false;
        };
        if v["final"].as_bool() == Some(true) && !ok {
            self.wait_end(id, true, &format!("Не выполнено: {text}"), tg)
                .await;
        }
        true
    }

    /// Конец ожидания: отметка записи снимается (free — кнопки снова доступны), сообщение вопроса — «… \n\n<итог>».
    /// Принимает: ID ожидания, снять ли отметку, строку итога, из Telegram ли. Ошибок нет.
    /// Ревью t193 (М7): WaitMark ещё нет, а ожидание из Telegram (tg) — итог в wait_ended, «Да» применит его сам.
    async fn wait_end(&self, id: &str, free: bool, line: &str, tg: bool) {
        let m = {
            let mut st = lock(&self.state);
            let m = st.waits.remove(id);
            if m.is_none() && tg {
                // Ревью t193 (М9): записи, которые «Да» так и не забрало, — не старше notify.dialog_ttl_s.
                if let Ok(s) = self
                    .notifier
                    .settings
                    .integer("notify.dialog_ttl_s", crate::settings::read::At::default())
                {
                    let ttl = std::time::Duration::from_secs(s);
                    st.wait_ended.retain(|_, (_, _, t)| t.elapsed() <= ttl);
                }
                st.wait_ended.insert(
                    id.to_string(),
                    (free, line.to_string(), std::time::Instant::now()),
                );
            }
            m
        };
        let Some(m) = m else {
            return;
        };
        if free && let Some(k) = &m.key {
            lock(&self.state).done.remove(k);
        }
        let n = &self.notifier;
        let ready = match client::build_role(&n.http, &n.settings, &n.vault, m.role) {
            Ok(r) => r,
            Err(e) => {
                return self
                    .log(
                        Level::Warn,
                        format!("Ожидание {id}: сообщение не исправлено: {}", e.message),
                    )
                    .await;
            }
        };
        let t = format!("{}\n\n{line}", m.text);
        if let Err(e) = ready.tg.edit_text_markup(&m.chat, m.msg, &t, None).await {
            self.log(
                Level::Warn,
                format!("Ожидание {id}: сообщение не исправлено: {e}"),
            )
            .await;
        }
    }
}
