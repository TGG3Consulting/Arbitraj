//! Файл: storage/journal_queue.rs
//! Что делает: решение 341 (задача t158; по решению владельца 16.11 — ВСЕГДА, без выключателя) — очередь записи
//! журнала основной базы: Journal::write (и строки отказов, решение 354) кладут операцию сюда и сразу возвращаются;
//! отдельный поток ОС (storage/journal_writer.rs; не пул блокирующих задач tokio) пишет накопленное одной транзакцией
//! на пачку по порядку постановки. Очередь одна на базу (Db): все журналы одной базы пишут в неё, поэтому чтение
//! журнала (Journal::list) дожидается записи всего, что поставлено до него (flushed) — «записал — прочитал» сохраняется.
//! Предел длины — journal.queue_max (необязательный; не задан — без предела, ничего не теряется): очередь полна —
//! запись не ставится, счёт потерь; следующая пачка сама добавляет запись journal.queue_lost (Warn) — не молча. Пачка
//! не записалась (ошибка SQLite) — её операции считаются потерянными (лог процесса и та же запись о потерях). Размер
//! пачки — не больше journal.batch_max (не задан — всё накопленное; учёт сделок в своём файле, решение 353, пачку не
//! ждёт). База закрывается (последний клон Db удалён) — очередь дописывается: shutdown ждёт поток-писатель.
//! Решение 354: метки повторяющихся строк выдаёт next_token в памяти.
//! Кто вызывает: storage/journal.rs, storage/journal_counted.rs (постановка), storage/journal_writer.rs (lose, done,
//! lost_note, batch_max), storage/db.rs (Inner: создание, shutdown при закрытии).
//! От чего зависит: storage/journal.rs (Entry, Level), storage/journal_op.rs, storage/journal_writer.rs,
//! std::sync::mpsc, std::thread, tokio::sync::Notify.
//! Настройки: journal.queue_max, journal.batch_max (передаёт storage/write_modes.rs через Journal).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use chrono::Utc;
use rusqlite::Connection;
use serde_json::json;
use tokio::sync::Notify;

use super::journal::{Entry, Level};
use super::journal_op::Op;

/// Вид записи журнала о потерянных записях (очередь полна или пачка не записалась).
pub const LOST_KIND: &str = "journal.queue_lost";

/// Очередь записи журнала одной базы.
#[derive(Default)]
pub struct Queue {
    /// Предел длины; 0 — без предела.
    max: AtomicUsize,
    /// Предел операций в одной транзакции пачки; 0 — всё накопленное.
    batch: AtomicUsize,
    /// Операций в очереди (поставлены, ещё не записаны).
    pending: AtomicUsize,
    /// Поставлено и записано (или потеряно пачкой) всего — граница для чтения (flushed).
    pushed: AtomicU64,
    written: AtomicU64,
    /// Потеряно с прошлой записи journal.queue_lost и всего с запуска.
    lost: AtomicU64,
    lost_total: AtomicU64,
    /// Отправитель потоку-писателю и сам поток (запускается при первой постановке).
    tx: Mutex<Option<Sender<Op>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// Решение 354: последняя выданная метка повторяющейся строки.
    token: AtomicU64,
    /// Сигнал «пачка записана».
    step: Notify,
}

/// Число из настройки (None — 0). Ошибок нет.
fn size(v: Option<u64>) -> usize {
    v.map_or(0, |m| usize::try_from(m).unwrap_or(usize::MAX))
}

impl Queue {
    /// Предел длины очереди (None или 0 — без предела). Ошибок нет.
    pub fn set_max(&self, max: Option<u64>) {
        self.max.store(size(max), Ordering::Release);
    }

    /// Предел операций в одной пачке (None или 0 — всё накопленное). Ошибок нет.
    pub fn set_batch_max(&self, max: Option<u64>) {
        self.batch.store(size(max), Ordering::Release);
    }

    /// Что делает: ставит операцию в очередь. Записи (Insert, Counted) подчиняются пределу journal.queue_max;
    /// обновления и «забыть» — нет (их число ограничено строками отказов; без них метка оторвалась бы от строки).
    /// Принимает: соединение основной базы (для запуска писателя), операцию. Возвращает: true — поставлена, false —
    /// очередь полна (потеря посчитана). Ошибок нет.
    pub(super) fn push_op(self: &Arc<Self>, conn: &Arc<Mutex<Connection>>, op: Op) -> bool {
        let limited = matches!(op, Op::Insert(_) | Op::Counted(..));
        let max = self.max.load(Ordering::Acquire);
        if limited && max > 0 && self.pending.load(Ordering::Acquire) >= max {
            self.lose(1, op.kind());
            return false;
        }
        let mut tx = self.tx.lock().unwrap_or_else(|p| p.into_inner());
        let sender = tx.get_or_insert_with(|| self.start(conn));
        self.pending.fetch_add(1, Ordering::AcqRel);
        self.pushed.fetch_add(1, Ordering::AcqRel);
        if let Err(back) = sender.send(op) {
            // Поток-писатель завершился (паника) — новый, операция — в него.
            let s = self.start(conn);
            if s.send(back.0).is_err() {
                self.done(1);
            }
            *tx = Some(s);
        }
        true
    }

    /// Решение 354: новая метка повторяющейся строки (в памяти, без базы). Ошибок нет.
    pub(super) fn next_token(&self) -> u64 {
        self.token.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Предел операций в одной пачке (0 — всё накопленное). Ошибок нет.
    pub(super) fn batch_max(&self) -> usize {
        self.batch.load(Ordering::Acquire)
    }

    /// Ждёт, пока очередь опустеет. Ошибок нет.
    pub async fn idle(&self) {
        self.wait(|q| q.pending.load(Ordering::Acquire) == 0).await;
    }

    /// Ждёт, пока записано всё, что поставлено до вызова (новые постановки не ждёт). Ошибок нет.
    pub async fn flushed(&self) {
        let target = self.pushed.load(Ordering::Acquire);
        self.wait(|q| q.written.load(Ordering::Acquire) >= target)
            .await;
    }

    /// Ждёт условия, проверяя его после каждой записанной пачки. Ошибок нет.
    async fn wait(&self, ok: impl Fn(&Self) -> bool) {
        loop {
            let wake = self.step.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            if ok(self) {
                return;
            }
            wake.await;
        }
    }

    /// Потеря n операций: счёт и лог процесса. Принимает: число, вид последней. Ошибок нет.
    pub(super) fn lose(&self, n: u64, kind: &str) {
        self.lost.fetch_add(n, Ordering::AcqRel);
        let total = self.lost_total.fetch_add(n, Ordering::AcqRel) + n;
        tracing::warn!(kind, total, "запись журнала потеряна (очередь журнала)");
    }

    /// n операций ушли из очереди (записаны или потеряны пачкой); сигнал ждущим. Ошибок нет.
    pub(super) fn done(&self, n: usize) {
        self.pending.fetch_sub(n, Ordering::AcqRel);
        self.written.fetch_add(n as u64, Ordering::AcqRel);
        self.step.notify_waiters();
    }

    /// Запись о потерях с прошлой (если были). Возвращает: запись и сколько она называет (вернуть счёт через unlose,
    /// если она сама не записалась — ревью Н5), или None. Ошибок нет.
    pub(super) fn lost_note(&self) -> Option<(Entry, u64)> {
        let since = self.lost.swap(0, Ordering::AcqRel);
        (since > 0).then(|| {
            let total = self.lost_total.load(Ordering::Acquire);
            let e = Entry {
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
            };
            (e, since)
        })
    }

    /// Запись о потерях не записалась — её счёт возвращается в «с прошлой записи». Принимает: число. Ошибок нет.
    pub(super) fn unlose(&self, n: u64) {
        self.lost.fetch_add(n, Ordering::AcqRel);
    }

    /// Запускает поток-писатель. Принимает: соединение основной базы. Возвращает: отправителя. Ошибок нет (поток не
    /// запустился — лог процесса; постановки дальше будут пробовать снова).
    fn start(self: &Arc<Self>, conn: &Arc<Mutex<Connection>>) -> Sender<Op> {
        let (tx, rx) = channel();
        let (conn, me) = (conn.clone(), Arc::downgrade(self));
        let r = std::thread::Builder::new()
            .name("journal-writer".into())
            .spawn(move || super::journal_writer::run(rx, conn, me));
        match r {
            Ok(h) => {
                let mut t = self.thread.lock().unwrap_or_else(|p| p.into_inner());
                // Прежний поток (паника) уже завершён — его ручку не ждём.
                *t = Some(h);
            }
            Err(e) => tracing::error!(%e, "поток записи журнала не запущен"),
        }
        tx
    }

    /// Что делает: база закрывается — дописать очередь: отправитель закрыт, поток-писатель дописывает полученное и
    /// завершается, вызывающий ждёт его. Вызов из самого писателя — без ожидания. Ошибок нет.
    pub(super) fn shutdown(&self) {
        drop(self.tx.lock().unwrap_or_else(|p| p.into_inner()).take());
        let h = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(h) = h
            && h.thread().id() != std::thread::current().id()
        {
            let _ = h.join();
        }
    }
}
