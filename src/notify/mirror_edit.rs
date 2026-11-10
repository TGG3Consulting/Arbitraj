//! Файл: notify/mirror_edit.rs
//! Что делает: часть дубля «Ручного управления» (решения 156, 177, 251 (б); вынесено из mirror_send.rs — размер файла):
//! правка сообщения сменившейся или вернувшейся записи (update_changed). Ответ (б) Тиграна 10.11: Telegram отказал в
//! правке (сообщение удалено, слишком старое и т. п.; Bot API тексты отказов не перечисляет — любой отказ ok=false
//! одинаково) — запись уходит новым сообщением, его id заменяет прежний (resend; в базе — mirror_store.rs persist).
//! Ответ 429 — пауза retry_after (ResponseParameters, https://core.telegram.org/bots/api#responseparameters); нет ответа
//! или связи — новое не шлётся (правка могла пройти), в журнал.
//! Кто вызывает: notify/mirror.rs (sync_tg).
//! От чего зависит: notify/mirror_state.rs, mirror_track.rs (replace_msg, set_text), mirror_view.rs, alert.rs,
//! tg_html.rs, tg_methods.rs, storage/journal.
//! Настройки: нет своих (пауза после ошибки — notify.updates_retry_ms через Mirror::tg_error).

use crate::core::types::ManualRecord;
use crate::storage::journal::Level;

use super::alert::telegram;
use super::mirror::Mirror;
use super::mirror_state::lock;
use super::mirror_view::record_message_for;
use super::telegram::{Telegram, TgError};
use super::tg_methods;

impl Mirror {
    /// Решение 177: сменившиеся и вернувшиеся записи (Diff.changed) — правка текста и кнопок первого сообщения записи.
    /// Принимает: клиента, (чат, показывать ли недоступные кнопки), записи. Ошибки — в журнал.
    pub(super) async fn update_changed(
        &self,
        tg: &Telegram,
        (chat, all): (&str, bool),
        recs: &[ManualRecord],
    ) {
        for r in recs {
            let Some(id) = lock(&self.state)
                .sent
                .get(&r.key)
                .and_then(|s| s.msgs.first().copied())
            else {
                continue;
            };
            let (text, kb) = record_message_for(r, all);
            let html = telegram(&text);
            match tg.edit_html(chat, id, &html, kb.clone()).await {
                Ok(()) => lock(&self.state).set_text(&r.key, text),
                // 429 — пауза retry_after (запись правится при следующем её изменении, как раньше).
                Err(e) if tg_methods::retry_after(&e).is_some() => {
                    self.tg_error(e, "правка сообщения записи").await
                }
                // Решение 251 (б): Telegram отказал в правке (сообщение удалено, слишком старое и т. п.) — новое
                // сообщение, запись теперь в нём. Тексты и коды отказов Bot API не перечисляет («error_code … subject to
                // change», https://core.telegram.org/bots/api#making-requests; editMessageText —
                // https://core.telegram.org/bots/api#editmessagetext) — любой отказ (ok=false) одинаково.
                Err(e @ TgError::Api { .. }) => {
                    self.resend(tg, chat, r, (text, html, kb), id, e).await
                }
                // Нет ответа или связи — правка могла пройти: новое не шлём (иначе дубль), в журнал.
                Err(e) => {
                    self.log(
                        Level::Warn,
                        format!("Ручное управление: сообщение {id} не обновлено: {e}"),
                    )
                    .await
                }
            }
        }
    }

    /// Решение 251 (б): правка отказана — запись уходит новым сообщением, его id заменяет прежний (и в базе — persist).
    /// Принимает: клиента, чат, запись, (текст, HTML, кнопки), id прежнего сообщения, отказ правки. Ошибки — в журнал.
    async fn resend(
        &self,
        tg: &Telegram,
        chat: &str,
        r: &ManualRecord,
        (text, html, kb): (String, String, Option<serde_json::Value>),
        old: i64,
        why: TgError,
    ) {
        match tg.send_html(chat, &html, kb).await {
            Ok(id) => {
                lock(&self.state).replace_msg(&r.key, text, id);
                let line = format!(
                    "Ручное управление: сообщение {old} не правится ({why}) — запись отправлена новым сообщением {id}"
                );
                self.log(Level::Info, line).await;
            }
            Err(e) => {
                self.tg_error(e, "сообщение записи (вместо неправленного)")
                    .await
            }
        }
    }
}
