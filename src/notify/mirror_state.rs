//! Файл: notify/mirror_state.rs
//! Что делает: память дубля «Ручного управления» в Telegram (решения 156, 157), общая для отправки (mirror.rs) и
//! приёма нажатий (press*.rs): какие записи уже отправлены и в каких сообщениях, какие ушли с экрана (ждут итога
//! «выполнено: кто, что, итог»), какие выполнены из Telegram (второе нажатие — «уже выполнено»), ожидающие
//! подтверждения «Да, …» и вопросы суммы (ForceReply), последнее напоминание и пауза по retry_after.
//! Логика решений — чистые функции без сети (проверяются тестами tests/notify_mirror.rs).
//! Решение 177: правка сообщения изменившейся или вернувшейся записи и итог продажи кнопкой — notify/mirror_track.rs.
//! Кто вызывает: notify/mirror.rs, notify/press.rs, notify/press_record.rs, notify/press_confirm.rs, notify/tg_commands.rs.
//! От чего зависит: core/types (ManualRecord, ManualAction).
//! Настройки: нет (интервал повтора notify.manual_repeat_s передаёт mirror.rs).
//!
//! Только в памяти: после перезапуска записи отправляются заново (сообщения прошлого запуска остаются с кнопками —
//! нажатие по ним идёт по текущим записям экрана: ушла — «уже выполнено»).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::core::types::{ManualAction, ManualRecord};

/// Сообщения записи в Telegram.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sent {
    /// Текст сообщения записи (для правки «выполнено: …»).
    pub text: String,
    /// id сообщений с кнопками этой записи (первое сообщение и вопросы подтверждения).
    pub msgs: Vec<i64>,
}

/// Ожидает подтверждения «Да, …».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// Ключ записи «Ручного управления» (None — команда плашки режима из меню).
    pub key: Option<String>,
    pub action: ManualAction,
    pub title: String,
    pub question: String,
}

/// Вопрос суммы (ForceReply): на какое сообщение ждём ответ — запись и код кнопки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub key: String,
    pub code: String,
}

/// Что изменилось в записях экрана.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Diff {
    /// Ушли с экрана: запись и её сообщения.
    pub gone: Vec<(ManualRecord, Sent)>,
    /// Решение 177: уже отправленные записи, чей текст сменился (тот же отказ — причина и попытка), и вернувшиеся с
    /// тем же ключом (продажа кнопкой не прошла) — их сообщение правится, нового нет (notify/mirror_track.rs).
    pub changed: Vec<ManualRecord>,
}

/// Память дубля.
#[derive(Debug, Default)]
pub struct MirrorState {
    /// Записи, которые сейчас на экране (по последнему снимку).
    pub known: HashMap<String, ManualRecord>,
    /// Отправленные записи → их сообщения.
    pub sent: HashMap<String, Sent>,
    /// Ушли с экрана, а итог команды ещё не пришёл: запись и её сообщения.
    pub closed: HashMap<String, (ManualRecord, Sent)>,
    /// Решение 177: ушли по команде продажи («выполнено: … запущена»), итог продажи ещё не пришёл — запись и её
    /// сообщения (notify/mirror_track.rs: правка «Выполнено: продано …» / «Не продано: …» или возврат записи).
    pub finished: HashMap<String, (ManualRecord, Sent)>,
    /// Выполнены из Telegram и ещё на экране — второе нажатие получает «уже выполнено».
    pub done: HashSet<String>,
    pub pending: HashMap<u64, Pending>,
    next: u64,
    /// (чат, id сообщения с ForceReply) → вопрос (id сообщения уникален только внутри чата, решение 176: чатов несколько).
    pub prompts: HashMap<(i64, i64), Prompt>,
    /// Последнее напоминание: id сообщения (его кнопки снимаются при следующем).
    pub reminder: Option<i64>,
    /// Когда начался отсчёт до следующего напоминания (None — ждущих нет).
    pub last_reminder: Option<Instant>,
    /// Telegram велел подождать (retry_after) или ошибка (notify.updates_retry_ms) — до этого момента не отправляем.
    pub pause_until: Option<Instant>,
    /// Последняя записанная в журнал ошибка — одна и та же не пишется повторно.
    pub last_error: Option<String>,
    /// Чат, в который ушли сообщения записей (решение 176: «Неудачные», если задан, иначе «Общий»).
    pub chat: Option<String>,
}

/// Начало номеров ожиданий и экранов — время запуска в мс: номер кнопки прошлого запуска не совпадёт с новым. Ошибок нет.
pub fn start_seed() -> u64 {
    static SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *SEED.get_or_init(|| chrono::Utc::now().timestamp_millis().unsigned_abs())
}

/// Общая память (дёшево клонируется).
pub type Shared = Arc<Mutex<MirrorState>>;

/// Замок памяти (отравленный замок — берём данные как есть: они не ломаются частичной записью).
pub fn lock(s: &Shared) -> MutexGuard<'_, MirrorState> {
    s.lock().unwrap_or_else(|p| p.into_inner())
}

impl MirrorState {
    /// Принимает новый снимок записей экрана. Возвращает: что ушло с экрана (с их сообщениями — для правки).
    /// Ушедшие убираются из done (запись с тем же ключом может вернуться — снова доступна) и переходят в closed.
    pub fn apply(&mut self, recs: &[ManualRecord]) -> Diff {
        let now: HashSet<&str> = recs.iter().map(|r| r.key.as_str()).collect();
        let gone_keys: Vec<String> = self
            .known
            .keys()
            .filter(|k| !now.contains(k.as_str()))
            .cloned()
            .collect();
        let mut diff = Diff::default();
        for k in gone_keys {
            let Some(rec) = self.known.remove(&k) else {
                continue;
            };
            self.done.remove(&k);
            if let Some(s) = self.sent.remove(&k) {
                self.closed.insert(k, (rec.clone(), s.clone()));
                diff.gone.push((rec, s));
            }
        }
        // Решение 177: сменившиеся и вернувшиеся записи — правка их сообщения (notify/mirror_track.rs).
        diff.changed = self.track(recs);
        for r in recs {
            self.known.insert(r.key.clone(), r.clone());
        }
        if self.waiting(recs).is_empty() {
            self.last_reminder = None;
        }
        diff
    }

    /// Чат зеркала сменился (задали или сняли чат «Неудачные», решение 176): прежние сообщения остаются в старом чате
    /// (их кнопки работают по текущим записям), отправленное забывается — записи уйдут в новый чат заново.
    /// Принимает: текущий чат зеркала. Возвращает: true — сменился. Ошибок нет.
    pub fn switch_chat(&mut self, chat: &str) -> bool {
        if self.chat.as_deref() == Some(chat) {
            return false;
        }
        let first = self.chat.is_none();
        self.chat = Some(chat.to_string());
        if !first {
            self.sent.clear();
            self.closed.clear();
            self.finished.clear();
            self.reminder = None;
            self.last_reminder = None;
        }
        !first
    }

    /// Записи, по которым ещё нет действия: на экране и не выполнены из Telegram. Порядок — как на экране.
    pub fn waiting<'a>(&self, recs: &'a [ManualRecord]) -> Vec<&'a ManualRecord> {
        recs.iter()
            .filter(|r| !self.done.contains(&r.key))
            .collect()
    }

    /// Решение 251: человек сейчас обрабатывает запись — открыт вопрос «Да/Отмена» по её кнопке (pending) или вопрос
    /// суммы (prompts). Такая запись не повторяется в напоминании. Принимает: ключ записи. Ошибок нет.
    pub fn busy(&self, key: &str) -> bool {
        self.pending.values().any(|p| p.key.as_deref() == Some(key))
            || self.prompts.values().any(|p| p.key == key)
    }

    /// Ещё не отправленные записи (из ждущих). Порядок — как на экране.
    pub fn unsent<'a>(&self, recs: &'a [ManualRecord]) -> Vec<&'a ManualRecord> {
        self.waiting(recs)
            .into_iter()
            .filter(|r| !self.sent.contains_key(&r.key))
            .collect()
    }

    /// Когда следующее напоминание. Принимает: интервал (None — notify.manual_repeat_s не задана).
    /// Возвращает: момент или None — повторов нет (не задано или некого напоминать).
    pub fn next_reminder(&self, every: Option<Duration>) -> Option<Instant> {
        Some(self.last_reminder? + every?)
    }

    /// Пора ли напоминать. Принимает: сейчас, интервал. Возвращает: true — пора (и не на паузе Telegram).
    pub fn reminder_due(&self, now: Instant, every: Option<Duration>) -> bool {
        let paused = self.pause_until.is_some_and(|p| now < p);
        !paused && self.next_reminder(every).is_some_and(|t| now >= t)
    }

    /// Отмечает отправку сообщения записи (и начало отсчёта напоминаний, если он не идёт).
    pub fn mark_sent(&mut self, key: &str, text: String, msg: i64, now: Instant) {
        let s = self.sent.entry(key.to_string()).or_default();
        s.text = text;
        s.msgs.push(msg);
        self.last_reminder.get_or_insert(now);
    }

    /// Кладёт ожидание подтверждения. Возвращает: номер для callback_data «y:<номер>».
    pub fn add_pending(&mut self, p: Pending) -> u64 {
        // Решение 176: номера — с времени запуска (start_seed): «Да» старого сообщения после перезапуска не совпадёт
        // с новым ожиданием и не выполнит чужое действие.
        self.next = self.next.max(start_seed()) + 1;
        self.pending.insert(self.next, p);
        self.next
    }

    /// Итог команды пришёл: ушедшие записи, к которым он относится, — их сообщения для правки «выполнено».
    /// Принимает: команду. Возвращает: (запись, сообщения) и убирает их из closed.
    pub fn take_closed(&mut self, a: &ManualAction) -> Vec<(ManualRecord, Sent)> {
        let keys: Vec<String> = self
            .closed
            .iter()
            .filter(|(_, (r, _))| r.matches(a))
            .map(|(k, _)| k.clone())
            .collect();
        keys.into_iter()
            .filter_map(|k| self.closed.remove(&k))
            .collect()
    }
}
