//! Файл: storage/journal_queue.rs
//! Что делает: решение 341 (задача t158) — очередь записи журнала: при заданной journal.queue_max Journal::write кладёт
//! запись сюда и сразу возвращается; отдельный поток ОС (не пул блокирующих задач tokio, общий со счётом котировок DEX)
//! пишет накопленное ОДНОЙ транзакцией по порядку постановки и затем отдаёт записи в живой поток интерфейса с
//! настоящим id. Размер пачки — не больше journal.batch_max записей (не задана — всё накопленное): транзакция пачки
//! держит замок основной базы, короткая пачка — короткое ожидание у остальных её пользователей. Очередь полна —
//! запись не ставится, счёт потерь; следующая пачка сама добавляет запись
//! journal.queue_lost (Warn: сколько потеряно с прошлой записи и всего) — не молча. Пачка не записалась (ошибка
//! SQLite) — её записи считаются потерянными (лог процесса и та же запись о потерях). Предел снят, а в очереди ещё
//! есть записи — новые тоже идут в очередь, пока она не опустеет (порядок не ломается).
//! Кто вызывает: storage/journal.rs (Journal::write, queued, set_queue_max, idle).
//! От чего зависит: storage/db.rs (call_sync), storage/journal.rs (Entry, insert), std::sync::mpsc, tokio::sync::Notify.
//! Настройки: journal.queue_max, journal.batch_max (передаёт storage/write_modes.rs через Journal::set_queue_max,
//! set_batch_max).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, Weak};

use chrono::Utc;
use serde_json::json;
use tokio::sync::{Notify, broadcast};

use super::db::Db;
use super::journal::{Entry, Level, insert};

/// Вид записи журнала о потерянных записях (очередь полна или пачка не записалась).
pub const LOST_KIND: &str = "journal.queue_lost";

/// Очередь записи журнала одного Journal (общая у его клонов).
#[derive(Default)]
pub struct Queue {
    /// Предел очереди; 0 — очередь выключена.
    max: AtomicUsize,
    /// Предел записей в одной транзакции пачки; 0 — всё накопленное.
    batch: AtomicUsize,
    /// Записей в очереди (поставлены, ещё не записаны).
    pending: AtomicUsize,
    /// Потеряно с прошлой записи journal.queue_lost и всего с запуска.
    lost: AtomicU64,
    lost_total: AtomicU64,
    /// Отправитель потоку-писателю (поток запускается при первой постановке).
    tx: Mutex<Option<Sender<Entry>>>,
    /// Сигнал «очередь опустела».
    idle: Notify,
}

impl Queue {
    /// Предел очереди (None или 0 — выключить). Ошибок нет.
    pub fn set_max(&self, max: Option<u64>) {
        let m = max.map_or(0, |m| usize::try_from(m).unwrap_or(usize::MAX));
        self.max.store(m, Ordering::Release);
    }

    /// Предел записей в одной пачке (None или 0 — всё накопленное). Ошибок нет.
    pub fn set_batch_max(&self, max: Option<u64>) {
        let m = max.map_or(0, |m| usize::try_from(m).unwrap_or(usize::MAX));
        self.batch.store(m, Ordering::Release);
    }

    /// Идёт ли запись через очередь: предел задан или в очереди ещё есть записи. Ошибок нет.
    pub fn on(&self) -> bool {
        self.max.load(Ordering::Acquire) > 0 || self.pending.load(Ordering::Acquire) > 0
    }

    /// Ставит запись в очередь. Принимает: базу и живой поток (для запуска писателя), запись.
    /// Возвращает: None — поставлена или потеряна со счётом (вызывающий не пишет); Some(запись) — очередь выключена,
    /// писать прямо. Ошибок нет.
    pub fn push(
        self: &Arc<Self>,
        db: &Db,
        live: &broadcast::Sender<Entry>,
        e: Entry,
    ) -> Option<Entry> {
        if !self.on() {
            return Some(e);
        }
        let max = self.max.load(Ordering::Acquire);
        if max > 0 && self.pending.load(Ordering::Acquire) >= max {
            self.lose(1, &e.kind);
            return None;
        }
        let mut tx = self.tx.lock().unwrap_or_else(|p| p.into_inner());
        let sender = tx.get_or_insert_with(|| self.start(db, live));
        self.pending.fetch_add(1, Ordering::AcqRel);
        if let Err(back) = sender.send(e) {
            // Поток-писатель завершился (паника) — новый, запись — в него.
            let s = self.start(db, live);
            if s.send(back.0).is_err() {
                self.done(1);
            }
            *tx = Some(s);
        }
        None
    }

    /// Ждёт, пока очередь опустеет. Ошибок нет.
    pub async fn idle(&self) {
        loop {
            let wake = self.idle.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            if self.pending.load(Ordering::Acquire) == 0 {
                return;
            }
            wake.await;
        }
    }

    /// Потеря n записей: счёт и лог процесса. Принимает: число, вид последней. Ошибок нет.
    fn lose(&self, n: u64, kind: &str) {
        self.lost.fetch_add(n, Ordering::AcqRel);
        let total = self.lost_total.fetch_add(n, Ordering::AcqRel) + n;
        tracing::warn!(
            kind,
            total,
            "запись журнала потеряна (очередь journal.queue_max)"
        );
    }

    /// n записей ушли из очереди; пусто — сигнал ждущим. Ошибок нет.
    fn done(&self, n: usize) {
        if self.pending.fetch_sub(n, Ordering::AcqRel) == n {
            self.idle.notify_waiters();
        }
    }

    /// Запись о потерях с прошлой (если были). Возвращает: запись или None. Ошибок нет.
    fn lost_note(&self) -> Option<Entry> {
        let since = self.lost.swap(0, Ordering::AcqRel);
        (since > 0).then(|| {
            let total = self.lost_total.load(Ordering::Acquire);
            Entry {
                id: 0,
                at: Utc::now(),
                level: Level::Warn,
                kind: LOST_KIND.to_string(),
                venue: None,
                message: format!(
                    "Журнал: потеряно {since} записей с прошлой такой записи (очередь journal.queue_max полна или пачка не записалась), всего с запуска {total}"
                ),
                data: Some(json!({ "lost": since, "total": total })),
                repeats: None,
                last_at: None,
            }
        })
    }

    /// Запускает поток-писатель. Принимает: базу, живой поток. Возвращает: отправителя. Ошибок нет (поток не
    /// запустился — лог процесса; записи будут считаться потерянными при следующей постановке).
    fn start(self: &Arc<Self>, db: &Db, live: &broadcast::Sender<Entry>) -> Sender<Entry> {
        let (tx, rx) = channel();
        let (db, live, me) = (db.clone(), live.clone(), Arc::downgrade(self));
        let r = std::thread::Builder::new()
            .name("journal-writer".into())
            .spawn(move || run(rx, db, live, me));
        if let Err(e) = r {
            tracing::error!(%e, "поток записи журнала не запущен");
        }
        tx
    }
}

/// Поток-писатель: ждёт первую запись, забирает накопленное (не больше journal.batch_max) и пишет одной транзакцией;
/// затем — в живой поток. Завершается, когда очередь (все клоны Journal) удалена. Принимает: приёмник, базу, живой поток, очередь. Ошибок нет.
fn run(rx: Receiver<Entry>, db: Db, live: broadcast::Sender<Entry>, q: Weak<Queue>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        let cap = q.upgrade().map_or(0, |q| q.batch.load(Ordering::Acquire));
        while cap == 0 || batch.len() < cap {
            let Ok(e) = rx.try_recv() else {
                break;
            };
            batch.push(e);
        }
        let taken = batch.len();
        if let Some(note) = q.upgrade().and_then(|q| q.lost_note()) {
            batch.push(note);
        }
        let res = db.call_sync(|c| {
            let t = c.transaction()?;
            for e in batch.iter_mut() {
                e.id = insert(&t, e)?;
            }
            t.commit()
        });
        let q = q.upgrade();
        match res {
            Ok(()) => {
                for e in batch {
                    // Нет слушателей — не ошибка: интерфейс может быть закрыт.
                    let _ = live.send(e);
                }
            }
            Err(err) => {
                tracing::error!(%err, n = batch.len(), "пачка журнала не записана");
                if let Some(q) = &q {
                    q.lose(taken as u64, "пачка");
                }
            }
        }
        if let Some(q) = q {
            q.done(taken);
        }
    }
}
