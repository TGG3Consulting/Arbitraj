//! Файл: notify/mirror_track.rs
//! Что делает: решение 177 (Тигран, 05.11.2026) — дубль «Ручного управления» в Telegram без дублей и с итогом продажи:
//! 1. Отправленная запись, чей текст сменился (тот же отказ продажи — та же запись с новой причиной и номером попытки,
//!    core/manual_repeat.rs), и запись, вернувшаяся с тем же ключом (продажа кнопкой не прошла), — правка её сообщения
//!    (текст и кнопки), а не новое сообщение. Запись снова ждёт действия (отметка «выполнено из Telegram» снимается).
//! 2. Запись ушла по команде продажи («Продать сейчас», «Продать и перегнать») — после правки «Выполнено: … запущена» её
//!    сообщения ждут итога продажи (событие {final: true, deal, ok, text} от core/manual_outcome.rs) и правятся ещё раз:
//!    «Выполнено: продано X по Y» или «Не продано: причина».
//!
//! Решение 251 (б): правка не прошла (Telegram отказал) — новое сообщение и его id (replace_msg, mirror_send.rs);
//! сообщения записей хранятся в базе (mirror_store.rs).
//! Кто вызывает: notify/mirror_state.rs (apply → track), notify/mirror.rs (правка сменившихся), notify/mirror_send.rs
//! (итог команды и итог продажи).
//! От чего зависит: notify/mirror_state.rs, core/types (ManualRecord, ManualAction).
//! Настройки: нет.

use crate::core::types::{ManualAction, ManualRecord};

use super::mirror_state::{MirrorState, Sent};

impl MirrorState {
    /// Сменившиеся и вернувшиеся записи снимка (вызывается из apply до обновления known). Вернувшиеся (ушли и снова на
    /// экране с тем же ключом) — их сообщения снова «отправленные». Принимает: снимок. Возвращает: записи к правке.
    pub(super) fn track(&mut self, recs: &[ManualRecord]) -> Vec<ManualRecord> {
        let mut out = Vec::new();
        for r in recs {
            if !self.sent.contains_key(&r.key) {
                let back = self
                    .closed
                    .remove(&r.key)
                    .or_else(|| self.finished.remove(&r.key));
                if let Some((_, s)) = back {
                    self.sent.insert(r.key.clone(), s);
                    self.done.remove(&r.key);
                    out.push(r.clone());
                }
            } else if self.known.get(&r.key).is_some_and(|k| k.text != r.text) {
                self.done.remove(&r.key);
                out.push(r.clone());
            }
        }
        out
    }

    /// Текст сообщения записи после правки (для следующей правки «выполнено»). Ошибок нет.
    pub fn set_text(&mut self, key: &str, text: String) {
        if let Some(s) = self.sent.get_mut(key) {
            s.text = text;
        }
    }

    /// Решение 251 (б): сообщение записи не правится (Telegram отказал: удалено, слишком старое и т. п.) — отправлено
    /// новое; теперь запись — в нём. Принимает: ключ, текст, id нового сообщения. Ошибок нет.
    pub fn replace_msg(&mut self, key: &str, text: String, msg: i64) {
        if let Some(s) = self.sent.get_mut(key) {
            s.text = text;
            s.msgs = vec![msg];
        }
    }

    /// Запись ушла по команде продажи, её сообщения уже правлены итогом команды — ждут итога продажи. Принимает: запись,
    /// сообщения с новым текстом. Ошибок нет.
    pub fn keep_finished(&mut self, rec: ManualRecord, sent: Sent) {
        self.finished.insert(rec.key.clone(), (rec, sent));
    }

    /// Итог продажи по сделке пришёл: сообщения записей этой сделки, ждавших его, — к правке; из памяти убираются.
    /// Принимает: id сделки. Возвращает: (запись, сообщения). Ошибок нет.
    pub fn take_finished(&mut self, deal: &str) -> Vec<(ManualRecord, Sent)> {
        let keys: Vec<String> = self
            .finished
            .iter()
            .filter(|(_, (r, _))| r.deal_id == deal)
            .map(|(k, _)| k.clone())
            .collect();
        keys.into_iter()
            .filter_map(|k| self.finished.remove(&k))
            .collect()
    }
}

/// Команда — продажа кнопкой, у которой будет итог продажи (core/manual_outcome.rs). Ошибок нет.
pub fn is_sale(a: &ManualAction) -> bool {
    matches!(
        a,
        ManualAction::SellNow { .. }
            | ManualAction::SellNowReturn { .. }
            | ManualAction::SellHalted { .. }
            | ManualAction::SellHaltedReturn { .. }
    )
}

/// Текст сообщения с итогом продажи. Принимает: прежний текст, итог («Выполнено: продано …» / «Не продано: …»).
pub fn final_text(old: &str, outcome: &str) -> String {
    format!("{old}\n\n{outcome}")
}
