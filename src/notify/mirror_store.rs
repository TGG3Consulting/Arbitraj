//! Файл: notify/mirror_store.rs
//! Что делает: решение 251, ответ (б) Тиграна 10.11 — память дубля «Ручного управления» переживает перезапуск: для
//! каждой отправленной записи, которая на экране, в SQLite (storage/notify_mirror.rs) лежит строка Row — чат, id
//! сообщений, сама запись (её текст сравнивается со снимком после перезапуска) и текст сообщения (для правки
//! «выполнено»). При запуске строки текущего чата зеркала возвращаются в память (MirrorState::restore): запись снова
//! на экране — её сообщение правится, если текст сменился (mirror_track.rs), иначе остаётся как есть, нового нет;
//! записи на экране уже нет — её сообщение правится «запись ушла» (mirror_state.rs apply). Строки чужого чата
//! (чат зеркала сменили, пока бот стоял) и нечитаемые — удаляются. Строка удаляется, когда запись ушла с экрана
//! (выполнена или закрыта) или чат зеркала сменился; после каждой сверки (Mirror::reconcile) база приводится к памяти
//! одной транзакцией — только изменившиеся строки.
//! Кто вызывает: notify/mirror.rs (run — restore при запуске, reconcile — persist).
//! От чего зависит: notify/mirror_state.rs, notify/roles.rs (чат зеркала), storage/notify_mirror.rs, core/types.
//! Настройки: ID чата зеркала (notify.failures_chat, если задан, иначе notify.telegram_chat — roles::mirror_role).

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::core::types::ManualRecord;
use crate::storage::journal::Level;
use crate::storage::notify_mirror;

use super::mirror::Mirror;
use super::mirror_state::{MirrorState, Sent, lock};

/// Строка базы: сообщения одной записи.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// Чат, в котором сообщения записи.
    pub chat: String,
    /// Запись, как она была при последней отправке или правке.
    pub rec: ManualRecord,
    /// Текст сообщения записи (без разметки).
    pub text: String,
    /// id сообщений записи с кнопками.
    pub msgs: Vec<i64>,
}

impl MirrorState {
    /// Какой должна быть база: отправленные записи, которые на экране. Принимает: —. Возвращает: ключ → JSON строки.
    /// Чат зеркала ещё не известен — пусто. Ошибок нет (запись, которую не удалось превратить в JSON, пропускается).
    pub fn rows(&self) -> HashMap<String, String> {
        let Some(chat) = self.chat.as_deref() else {
            return HashMap::new();
        };
        self.sent
            .iter()
            .filter_map(|(k, s)| {
                let rec = self.known.get(k)?.clone();
                let row = Row {
                    chat: chat.to_string(),
                    rec,
                    text: s.text.clone(),
                    msgs: s.msgs.clone(),
                };
                Some((k.clone(), serde_json::to_string(&row).ok()?))
            })
            .collect()
    }

    /// Что записать и что удалить, чтобы база стала как rows(). Принимает: нужное состояние.
    /// Возвращает: (записать [(ключ, JSON)], удалить [ключ]). Ошибок нет.
    pub fn store_plan(
        &self,
        want: &HashMap<String, String>,
    ) -> (Vec<(String, String)>, Vec<String>) {
        let put = want
            .iter()
            .filter(|(k, v)| self.stored.get(*k) != Some(*v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let del = self
            .stored
            .keys()
            .filter(|k| !want.contains_key(*k))
            .cloned()
            .collect();
        (put, del)
    }

    /// Строки базы после запуска → память. Принимает: строки [(ключ, JSON)], текущий чат зеркала, сейчас.
    /// Возвращает: ключи строк к удалению (другой чат или нечитаемые). Отсчёт напоминаний начинается с запуска.
    /// Ошибок нет.
    pub fn restore(
        &mut self,
        rows: Vec<(String, String)>,
        chat: &str,
        now: Instant,
    ) -> Vec<String> {
        let mut del = Vec::new();
        for (key, body) in rows {
            match serde_json::from_str::<Row>(&body) {
                Ok(r) if r.chat == chat && r.rec.key == key => {
                    self.known.insert(key.clone(), r.rec);
                    let sent = Sent {
                        text: r.text,
                        msgs: r.msgs,
                    };
                    self.sent.insert(key.clone(), sent);
                    self.stored.insert(key, body);
                }
                _ => del.push(key),
            }
        }
        self.chat = Some(chat.to_string());
        if !self.sent.is_empty() {
            self.last_reminder = Some(now);
        }
        del
    }
}

impl Mirror {
    /// При запуске: строки базы текущего чата зеркала → память; чужие и нечитаемые — удаляются. Чат зеркала не задан —
    /// база не трогается (строки дождутся чата при следующем запуске). Ошибки — в журнал.
    pub(super) async fn restore(&self) {
        let role = super::roles::mirror_role(&self.notifier.settings);
        let Some(chat) = super::roles::chat(&self.notifier.settings, role) else {
            return;
        };
        let rows = match notify_mirror::load_all(&self.db).await {
            Ok(r) => r,
            Err(e) => {
                let why =
                    format!("Ручное управление: сообщения прошлого запуска не прочитаны: {e}");
                return self.fail(why).await;
            }
        };
        let n = rows.len();
        let del = lock(&self.state).restore(rows, &chat, Instant::now());
        if n > 0 {
            let kept = n - del.len();
            let text = format!(
                "Ручное управление: сообщений записей прошлого запуска — {kept} (правятся, а не отправляются заново); удалено строк чужого чата — {}",
                del.len()
            );
            self.log(Level::Info, text).await;
        }
        if !del.is_empty()
            && let Err(e) = notify_mirror::apply(&self.db, Vec::new(), del).await
        {
            self.fail(format!("Ручное управление: строки базы не удалены: {e}"))
                .await;
        }
    }

    /// После сверки: база — как память (только изменившиеся строки, одной транзакцией). Ошибки — в журнал (одна и та
    /// же — один раз подряд); при ошибке память о сохранённом не меняется — следующая сверка повторит запись.
    pub(super) async fn persist(&self) {
        let (want, put, del) = {
            let st = lock(&self.state);
            let want = st.rows();
            let (put, del) = st.store_plan(&want);
            (want, put, del)
        };
        if put.is_empty() && del.is_empty() {
            return;
        }
        match notify_mirror::apply(&self.db, put, del).await {
            Ok(()) => lock(&self.state).stored = want,
            Err(e) => {
                self.fail(format!(
                    "Ручное управление: сообщения записей не сохранены в базе: {e}"
                ))
                .await
            }
        }
    }
}
