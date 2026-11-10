//! Файл: notify/mirror.rs
//! Что делает: дубль экрана «Ручное управление» в Telegram (решение 156). Каждая запись экрана при появлении уходит
//! сообщением (событие manual.record) с теми же кнопками; пока по записям нет действия — каждые notify.manual_repeat_s
//! секунд ОДНО напоминание пачкой (список ждущих записей и кнопки каждой; у прежнего напоминания кнопки снимаются).
//! Запись ушла с экрана (действие с экрана, из Telegram или само разрешилось) — её сообщения правятся «запись ушла»,
//! кнопки убираются, повторов больше нет; пришёл итог команды — «Выполнено: кто, что, итог».
//! Лимиты Telegram: в устоявшемся режиме — не больше двух запросов (одно новое сообщение и одна правка) за интервал
//! повтора, сколько бы ни было записей; ответ 429 — пауза ровно на parameters.retry_after
//! (ResponseParameters, https://core.telegram.org/bots/api#responseparameters).
//! Решение 176: чат зеркала — «Неудачные» (notify.failures_chat), если задан, иначе «Общий» (как раньше, roles.rs).
//! Решение 177: запись сменилась или вернулась с тем же ключом — её сообщение правится, нового нет; итог продажи
//! кнопкой — ещё одна правка сообщения ушедшей записи (mirror_track.rs, mirror_send.rs).
//! Решение 251: тексты — по шаблону алерта в HTML (notify/alert.rs::telegram, карточка сделки — путь и где сейчас);
//! напоминание не повторяет записи, которые человек сейчас обрабатывает (вопрос «Да/Отмена» или вопрос суммы открыт —
//! mirror_state.rs::busy). Ответ (б) Тиграна 10.11: сообщения записей — в SQLite, после перезапуска правятся, а не
//! отправляются заново (mirror_store.rs: restore при запуске, persist после каждой сверки).
//! Кто вызывает: main.rs (задача run). Напоминание, итог команды и ошибки — mirror_send.rs; правка сменившихся — mirror_edit.rs.
//! От чего зависит: core (Core::manual_records, Core::subscribe), notify/mirror_state.rs, mirror_view.rs,
//! tg_methods.rs, tg_html.rs, alert.rs, client.rs, dispatch.rs (allows), mirror_store.rs, settings, storage/journal,
//! storage/db (база для mirror_store.rs).
//! Настройки: notify.manual_repeat_s (нет — без повторов), notify.updates_retry_ms (пауза после ошибки Telegram),
//! notify.event.* для manual.record, notify.telegram_*; секрет telegram.bot_token.

use std::time::{Duration, Instant};

use tokio::sync::broadcast::error::RecvError;

use crate::core::Core;
use crate::core::types::ManualRecord;
use crate::core::view::CoreEvent;
use crate::settings::live::Watch;
use crate::settings::read::At;
use crate::storage::db::Db;
use crate::storage::journal::Level;

use super::alert::telegram;
use super::client;
use super::dispatch::Notifier;
use super::mirror_state::{Shared, lock};
use super::mirror_view::{closed_text, record_message_for};

/// Код события дубля в notify/catalog.rs.
pub const EVENT: &str = "manual.record";

/// Дубль «Ручного управления».
pub struct Mirror {
    pub core: Core,
    pub notifier: Notifier,
    pub state: Shared,
    /// База SQLite — сообщения записей переживают перезапуск (решение 251 (б), mirror_store.rs).
    pub db: Db,
}

/// Работает, пока система запущена. Принимает: дубль и подписку на настройки (notify.*, http.timeout_ms).
/// Ошибок не возвращает: всё пишется в журнал.
pub async fn run(m: Mirror, mut watch: Watch) {
    let mut rx = m.core.subscribe();
    // Решение 251 (б): сообщения записей прошлого запуска — в память до первой сверки (правятся, не шлются заново).
    m.restore().await;
    loop {
        m.reconcile().await;
        let every = m.every();
        let wake = {
            let st = lock(&m.state);
            // На паузе Telegram — ждём её конца, а не прошедшего срока напоминания (иначе — пустой круг).
            match st.pause_until {
                Some(p) if Instant::now() < p => Some(p),
                _ => st.next_reminder(every),
            }
        };
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(CoreEvent::Command(v)) => {
                    m.reconcile().await;
                    m.outcome(&v).await;
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
            _ = sleep_until(wake) => {}
            c = watch.next() => if c.is_none() { return },
        }
    }
}

/// Ждёт до момента (None — никогда).
async fn sleep_until(t: Option<Instant>) {
    match t {
        Some(t) => tokio::time::sleep_until(t.into()).await,
        None => std::future::pending().await,
    }
}

impl Mirror {
    /// Интервал напоминаний notify.manual_repeat_s. None — не задан (без повторов).
    pub(super) fn every(&self) -> Option<Duration> {
        let s = &self.notifier.settings;
        s.integer("notify.manual_repeat_s", At::default())
            .ok()
            .map(Duration::from_secs)
    }

    /// Сверка (sync_tg) и сохранение сообщений записей в базу (решение 251 (б), mirror_store.rs). Ошибок не возвращает.
    async fn reconcile(&self) {
        self.sync_tg().await;
        self.persist().await;
    }

    /// Сверяет записи экрана с отправленным: правит ушедшие, отправляет новые, напоминает. Ошибок не возвращает.
    async fn sync_tg(&self) {
        let recs = self.core.manual_records();
        let diff = lock(&self.state).apply(&recs);
        let role = super::roles::mirror_role(&self.notifier.settings);
        let ready = match client::build_role(
            &self.notifier.http,
            &self.notifier.settings,
            &self.notifier.vault,
            role,
        ) {
            Ok(r) => r,
            Err(e) => {
                let busy = !lock(&self.state).unsent(&recs).is_empty() || !diff.gone.is_empty();
                // Срок напоминания сдвигается на интервал — без клиента не крутимся впустую.
                if let Some(t) = lock(&self.state).last_reminder.as_mut() {
                    *t = Instant::now();
                }
                if busy
                    && recs
                        .iter()
                        .any(|r| self.notifier.allows(EVENT, Some(&r.venue)))
                {
                    self.fail(format!(
                        "Ручное управление в Telegram не отправлено: {}",
                        e.message
                    ))
                    .await;
                }
                return;
            }
        };
        let (tg, chat) = (&ready.tg, ready.chat.as_str());
        let all = role == super::roles::Role::Failures;
        if lock(&self.state).switch_chat(chat) {
            self.log(
                Level::Info,
                format!(
                    "Ручное управление: записи — теперь в чат «{}»",
                    role.title()
                ),
            )
            .await;
        }
        for (_, sent) in &diff.gone {
            for id in &sent.msgs {
                let t = telegram(&closed_text(&sent.text, None));
                if let Err(e) = tg.edit_html(chat, *id, &t, None).await {
                    self.log(
                        Level::Warn,
                        format!("Ручное управление: сообщение {id} не исправлено: {e}"),
                    )
                    .await;
                }
            }
        }
        // Решение 177: сменившиеся и вернувшиеся записи — правка их сообщения, не новое (mirror_track.rs).
        self.update_changed(tg, (chat, all), &diff.changed).await;
        let now = Instant::now();
        if lock(&self.state).pause_until.is_some_and(|p| now < p) {
            return;
        }
        let allowed: Vec<&ManualRecord> = recs
            .iter()
            .filter(|r| self.notifier.allows(EVENT, Some(&r.venue)))
            .collect();
        let unsent: Vec<ManualRecord> = {
            let st = lock(&self.state);
            st.unsent(&recs)
                .into_iter()
                .filter(|r| allowed.iter().any(|a| a.key == r.key))
                .cloned()
                .collect()
        };
        for r in &unsent {
            let (text, kb) = record_message_for(r, all);
            match tg.send_html(chat, &telegram(&text), kb).await {
                Ok(id) => {
                    lock(&self.state).mark_sent(&r.key, text, id, Instant::now());
                    // Решение 251: полный текст причины — в журнал (в сообщении — коротко, «полностью — в журнале»).
                    self.log_full(r).await;
                }
                Err(e) => return self.tg_error(e, "сообщение записи").await,
            }
        }
        self.remind(tg, (chat, all), &recs, &allowed).await;
        lock(&self.state).last_error = None;
    }
}
