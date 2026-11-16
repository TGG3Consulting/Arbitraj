//! Файл: notify/queue.rs
//! Что делает: решение 340 (Тигран 16.11: «Алерт в Telegram — это параллельное, информативное, с процессом сделки
//! никак не связанное») — очередь уведомлений. Notifier::send / send_tagged / send_with_buttons / later кладут задание
//! сюда и сразу возвращаются: ни один шаг сделки не ждёт Telegram. Решение 356 (ревью t158 Н1): две полосы на рантайм
//! tokio, у каждой своя фоновая задача — «важная» (группы каталога «Риск» и «Ручное управление»: алерты риска и
//! записи ручного управления) и «обычная» (всё остальное, в том числе события сделок); при лежащем Telegram важные не
//! стоят за хвостом обычных. Внутри полосы задания — строго по порядку постановки: порядок уведомлений одной сделки
//! (куплено → переведено → зачислено → продано → закрыта — все в обычной полосе) сохраняется. Каждое задание — своей
//! задачей tokio (паника одного не останавливает полосу). Карточка сделки снимается при постановке (alert_card.rs
//! snap/scoped) — содержимое на момент события.
//! Ёмкость — notify.queue_max (обе полосы вместе; не задана — без предела, ничего не теряется). Очередь полна —
//! задание не ставится, счёт потерь и запись журнала notify.queue_lost (Warn) на 1-й, 2-й, 4-й… потере (не молча).
//! Устаревание — notify.info_max_age_s (только обычная полоса): задание ждало дольше — не выполняется, та же потеря.
//! Кто вызывает: notify/dispatch.rs, notify/dispatch_role.rs (постановка), core/deal_closed.rs (Notifier::later),
//! core/shutdown.rs (idle, pending), тесты (idle).
//! От чего зависит: tokio (mpsc, Notify, Handle), notify/alert_card.rs, notify/dispatch.rs (body), notify/catalog.rs,
//! settings, storage/journal.rs.
//! Настройки: notify.queue_max, notify.info_max_age_s (необязательные).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::runtime::{Handle, Id};
use tokio::sync::{Notify, mpsc};

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::alert_card::{self, Snap};
use super::dispatch::Notifier;

/// Ёмкость очереди, сообщений (решение 340).
pub const MAX: &str = "notify.queue_max";
/// Предел ожидания задания обычной полосы, секунды (решение 356).
pub const MAX_AGE: &str = "notify.info_max_age_s";
/// Вид записи журнала о потере уведомления (очередь полна, устарело).
pub const LOST_KIND: &str = "notify.queue_lost";
/// Группы каталога событий, которые идут важной полосой (решение 356: алерты риска и ручное управление).
const PRIORITY_GROUPS: &[&str] = &["Риск", "Ручное управление"];

/// Задание очереди.
type Job = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Задание с отметкой постановки и тем, что нужно для записи о потере при устаревании.
struct Item {
    job: Job,
    at: Instant,
    n: Notifier,
    code: String,
    venue: Option<String>,
}

/// Счёт заданий обеих полос: сколько ждут или выполняются, сигнал «пусто».
#[derive(Default)]
struct State {
    pending: AtomicUsize,
    idle: Notify,
}

/// Очередь одного рантайма: две полосы (важная, обычная) и общий счёт.
struct Queue {
    urgent: mpsc::UnboundedSender<Box<Item>>,
    normal: mpsc::UnboundedSender<Box<Item>>,
    state: Arc<State>,
}

/// Очереди по рантаймам tokio (в работе бота рантайм один; у тестов — свой у каждого).
static QUEUES: LazyLock<Mutex<HashMap<Id, Queue>>> = LazyLock::new(Default::default);
/// Потеряно уведомлений с запуска (очередь полна, устарело или нет рантайма).
static LOST: AtomicU64 = AtomicU64::new(0);

/// Снимок карточки сделки для уведомления code с текстом text — сейчас, при постановке (тот же разбор, что у
/// отправки: dispatch::body → «Сделка: <id>»). Неизвестный код — снимка нет. Ошибок нет.
pub(super) fn snap(code: &str, text: &str) -> Snap {
    match super::catalog::find(code) {
        Some(def) => alert_card::snap(&super::dispatch::body(code, def, text)),
        None => Snap::default(),
    }
}

/// Важное ли событие (группа каталога — в PRIORITY_GROUPS). Неизвестный код — обычное. Ошибок нет.
fn urgent(code: &str) -> bool {
    super::catalog::find(code).is_some_and(|d| PRIORITY_GROUPS.contains(&d.group))
}

/// Ставит задание в очередь своего рантайма (полоса — по группе события); возвращается сразу. Принимает:
/// отправителя (настройки и журнал — для предела и счёта потерь), код события и площадку, снимок карточки, задание.
/// Ошибок нет.
pub(super) fn push(n: &Notifier, code: &str, venue: Option<&str>, snap: Snap, job: Job) {
    let Ok(rt) = Handle::try_current() else {
        return lost(n, code, venue, "нет рантайма tokio");
    };
    let max = n
        .settings
        .integer(MAX, At::default())
        .ok()
        .filter(|m| *m > 0);
    let item = Box::new(Item {
        job: Box::pin(alert_card::scoped(snap, job)),
        at: Instant::now(),
        n: n.clone(),
        code: code.to_string(),
        venue: venue.map(str::to_string),
    });
    let fast = urgent(code);
    let mut all = QUEUES.lock().unwrap_or_else(|p| p.into_inner());
    let q = all.entry(rt.id()).or_insert_with(|| start(&rt));
    if let Some(m) = max
        && q.state.pending.load(Ordering::Acquire) as u64 >= m
    {
        drop(all);
        return lost(n, code, venue, &format!("очередь полна ({MAX} = {m})"));
    }
    q.state.pending.fetch_add(1, Ordering::AcqRel);
    let sent = if fast {
        q.urgent.send(item)
    } else {
        q.normal.send(item)
    };
    if let Err(back) = sent {
        // Фоновые задачи завершились (рантайм останавливается) — новая очередь; задание — в неё.
        q.state.pending.fetch_sub(1, Ordering::AcqRel);
        *q = start(&rt);
        q.state.pending.fetch_add(1, Ordering::AcqRel);
        let again = if fast {
            q.urgent.send(back.0)
        } else {
            q.normal.send(back.0)
        };
        if again.is_err() {
            q.state.pending.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Запускает обе полосы в рантайме. Принимает: рантайм. Возвращает: очередь. Ошибок нет.
fn start(rt: &Handle) -> Queue {
    let state = Arc::new(State::default());
    let (urgent, rx_u) = mpsc::unbounded_channel::<Box<Item>>();
    let (normal, rx_n) = mpsc::unbounded_channel::<Box<Item>>();
    rt.spawn(lane(rx_u, state.clone(), false));
    rt.spawn(lane(rx_n, state.clone(), true));
    Queue {
        urgent,
        normal,
        state,
    }
}

/// Полоса: задания по порядку, каждое — своей задачей. aging — проверять notify.info_max_age_s (обычная полоса).
/// Принимает: приёмник, счёт, признак. Ошибок нет.
async fn lane(mut rx: mpsc::UnboundedReceiver<Box<Item>>, st: Arc<State>, aging: bool) {
    while let Some(item) = rx.recv().await {
        let Item {
            job,
            at,
            n,
            code,
            venue,
        } = *item;
        let limit = aging
            .then(|| n.settings.integer(MAX_AGE, At::default()).ok())
            .flatten();
        match limit {
            Some(s) if at.elapsed() > Duration::from_secs(s) => {
                drop(job);
                lost(
                    &n,
                    &code,
                    venue.as_deref(),
                    &format!("ждало дольше {MAX_AGE} = {s} с"),
                );
            }
            // Своей задачей: паника задания не останавливает полосу; следующее — только после этого (порядок).
            _ => {
                if let Err(e) = tokio::spawn(job).await {
                    tracing::error!(%e, "задание очереди уведомлений завершилось сбоем");
                }
            }
        }
        if st.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            st.idle.notify_waiters();
        }
    }
}

/// Сколько уведомлений своего рантайма ждут или выполняются (остановка — запись о неотправленных). Ошибок нет.
pub fn pending() -> usize {
    let Ok(rt) = Handle::try_current() else {
        return 0;
    };
    let all = QUEUES.lock().unwrap_or_else(|p| p.into_inner());
    all.get(&rt.id())
        .map_or(0, |q| q.state.pending.load(Ordering::Acquire))
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
