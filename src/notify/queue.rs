//! Файл: notify/queue.rs
//! Что делает: решение 340 (Тигран 16.11: «Алерт в Telegram — это параллельное, информативное, с процессом сделки
//! никак не связанное») — очередь уведомлений. Notifier::send / send_tagged / send_with_buttons / later кладут задание
//! сюда и сразу возвращаются: ни один шаг сделки не ждёт Telegram. Одна фоновая задача на рантайм tokio выполняет
//! задания строго по порядку постановки — порядок уведомлений одной сделки (куплено → переведено → зачислено →
//! продано → закрыта) сохраняется. Каждое задание — своей задачей tokio (паника одного не останавливает очередь).
//! Карточка сделки снимается при постановке (alert_card.rs snap/scoped) — содержимое на момент события.
//! Ёмкость — notify.queue_max (не задана — без предела, ничего не теряется). Очередь полна — задание не ставится,
//! счёт потерь и запись журнала notify.queue_lost (Warn) на 1-й, 2-й, 4-й… потере (не молча); сделка не ждёт ни
//! в одном случае.
//! Кто вызывает: notify/dispatch.rs, notify/dispatch_role.rs (постановка), core/deal_closed.rs (Notifier::later),
//! тесты (idle).
//! От чего зависит: tokio (mpsc, Notify, Handle), notify/alert_card.rs, notify/dispatch.rs (body), notify/catalog.rs,
//! settings, storage/journal.rs.
//! Настройки: notify.queue_max (необязательная).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::json;
use tokio::runtime::{Handle, Id};
use tokio::sync::{Notify, mpsc};

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::alert_card::{self, Snap};
use super::dispatch::Notifier;

/// Ёмкость очереди, сообщений (решение 340).
pub const MAX: &str = "notify.queue_max";
/// Вид записи журнала о потере уведомления (очередь полна).
pub const LOST_KIND: &str = "notify.queue_lost";

/// Задание очереди.
type Job = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Счёт заданий очереди: сколько ждут или выполняются, сигнал «пусто».
#[derive(Default)]
struct State {
    pending: AtomicUsize,
    idle: Notify,
}

/// Очередь одного рантайма: отправитель заданий фоновой задаче и её счёт.
struct Queue {
    tx: mpsc::UnboundedSender<Job>,
    state: Arc<State>,
}

/// Очереди по рантаймам tokio (в работе бота рантайм один; у тестов — свой у каждого).
static QUEUES: LazyLock<Mutex<HashMap<Id, Queue>>> = LazyLock::new(Default::default);
/// Потеряно уведомлений с запуска (очередь полна или нет рантайма).
static LOST: AtomicU64 = AtomicU64::new(0);

/// Снимок карточки сделки для уведомления code с текстом text — сейчас, при постановке (тот же разбор, что у
/// отправки: dispatch::body → «Сделка: <id>»). Неизвестный код — снимка нет. Ошибок нет.
pub(super) fn snap(code: &str, text: &str) -> Snap {
    match super::catalog::find(code) {
        Some(def) => alert_card::snap(&super::dispatch::body(code, def, text)),
        None => Snap::default(),
    }
}

/// Ставит задание в очередь своего рантайма; возвращается сразу. Принимает: отправителя (настройки и журнал — для
/// предела и счёта потерь), код события и площадку (для записи о потере), снимок карточки, задание. Ошибок нет.
pub(super) fn push(n: &Notifier, code: &str, venue: Option<&str>, snap: Snap, job: Job) {
    let Ok(rt) = Handle::try_current() else {
        return lost(n, code, venue, "нет рантайма tokio");
    };
    let max = n
        .settings
        .integer(MAX, At::default())
        .ok()
        .filter(|m| *m > 0);
    let job: Job = Box::pin(alert_card::scoped(snap, job));
    let mut all = QUEUES.lock().unwrap_or_else(|p| p.into_inner());
    let q = all.entry(rt.id()).or_insert_with(|| start(&rt));
    if let Some(m) = max
        && q.state.pending.load(Ordering::Acquire) as u64 >= m
    {
        drop(all);
        return lost(n, code, venue, &format!("очередь полна ({MAX} = {m})"));
    }
    q.state.pending.fetch_add(1, Ordering::AcqRel);
    if let Err(back) = q.tx.send(job) {
        // Фоновая задача завершилась (рантайм останавливается) — новая очередь; задание — в неё.
        q.state.pending.fetch_sub(1, Ordering::AcqRel);
        *q = start(&rt);
        q.state.pending.fetch_add(1, Ordering::AcqRel);
        if q.tx.send(back.0).is_err() {
            q.state.pending.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Запускает фоновую задачу очереди в рантайме. Принимает: рантайм. Возвращает: очередь. Ошибок нет.
fn start(rt: &Handle) -> Queue {
    let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
    let state = Arc::new(State::default());
    let st = state.clone();
    rt.spawn(async move {
        while let Some(job) = rx.recv().await {
            // Своей задачей: паника задания не останавливает очередь; следующее — только после этого (порядок).
            if let Err(e) = tokio::spawn(job).await {
                tracing::error!(%e, "задание очереди уведомлений завершилось сбоем");
            }
            if st.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                st.idle.notify_waiters();
            }
        }
    });
    Queue { tx, state }
}

/// Ждёт, пока очередь своего рантайма опустеет (все поставленные задания выполнены). Для тестов и остановки.
/// Ошибок нет; очереди нет — сразу.
pub async fn idle() {
    let Ok(rt) = Handle::try_current() else {
        return;
    };
    let st = {
        let all = QUEUES.lock().unwrap_or_else(|p| p.into_inner());
        all.get(&rt.id()).map(|q| q.state.clone())
    };
    let Some(st) = st else {
        return;
    };
    loop {
        let wake = st.idle.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        if st.pending.load(Ordering::Acquire) == 0 {
            return;
        }
        wake.await;
    }
}

/// Потеря уведомления: счёт, лог процесса и запись журнала notify.queue_lost на 1-й, 2-й, 4-й… потере (отдельной
/// задачей; нет рантайма — только лог процесса). Принимает: отправителя, код, площадку, причину. Ошибок нет.
fn lost(n: &Notifier, code: &str, venue: Option<&str>, why: &str) {
    let total = LOST.fetch_add(1, Ordering::Relaxed) + 1;
    tracing::warn!(
        code,
        venue,
        total,
        "уведомление не поставлено в очередь: {why}"
    );
    if !total.is_power_of_two() {
        return;
    }
    let Ok(rt) = Handle::try_current() else {
        return;
    };
    let msg = format!(
        "Уведомление «{code}» не отправлено: {why}; всего потеряно с запуска {total} (запись на 1-й, 2-й, 4-й… потере)"
    );
    let data = json!({ "code": code, "total": total });
    let (journal, venue) = (n.journal.clone(), venue.map(str::to_string));
    rt.spawn(async move {
        let _ = journal
            .write(Level::Warn, LOST_KIND, venue.as_deref(), msg, Some(data))
            .await;
    });
}

/// Сколько уведомлений потеряно с запуска (очередь полна или нет рантайма). Ошибок нет.
pub fn lost_total() -> u64 {
    LOST.load(Ordering::Relaxed)
}
