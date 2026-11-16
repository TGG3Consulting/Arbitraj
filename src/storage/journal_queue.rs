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
//! Решение 354: кроме записей — операции повторяющихся строк (journal_op.rs: строка с меткой, обновление, «забыть»),
//! метки выдаёт next_token в памяти. Сам поток-писатель — storage/journal_writer.rs.
//! Кто вызывает: storage/journal.rs (Journal::write, queued, set_queue_max, idle), storage/journal_counted.rs,
//! storage/journal_writer.rs (lose, done, lost_note, batch_max).
//! От чего зависит: storage/db.rs, storage/journal.rs (Entry), storage/journal_op.rs, storage/journal_writer.rs,
//! std::sync::mpsc, tokio::sync::Notify.
//! Настройки: journal.queue_max, journal.batch_max (передаёт storage/write_modes.rs через Journal::set_queue_max,
//! set_batch_max).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde_json::json;
use tokio::sync::{Notify, broadcast};

use super::db::Db;
use super::journal::{Entry, Level};
use super::journal_op::Op;

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
    tx: Mutex<Option<Sender<Op>>>,
    /// Решение 354: последняя выданная метка повторяющейся строки (journal_counted.rs).
    token: AtomicU64,
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
        match self.push_op(db, live, Op::Insert(Box::new(e)), false) {
            Pushed::Off(Op::Insert(e)) => Some(*e),
            _ => None,
        }
    }

    /// Что делает: ставит операцию в очередь. force — ставить, даже если очередь выключена (операции по метке:
    /// карту меток знает только писатель), и без предела journal.queue_max для обновлений и «забыть» (их число
    /// ограничено строками отказов; предел защищает от шквала записей). Принимает: базу, живой поток, операцию, force.
    /// Возвращает: Queued, Lost (предел, счёт потерь) или Off(операция) — очередь выключена, выполнять прямо.
    /// Ошибок нет.
    pub(super) fn push_op(
        self: &Arc<Self>,
        db: &Db,
        live: &broadcast::Sender<Entry>,
        op: Op,
        force: bool,
    ) -> Pushed {
        if !force && !self.on() {
            return Pushed::Off(op);
        }
        let limited = matches!(op, Op::Insert(_) | Op::Counted(..));
        let max = self.max.load(Ordering::Acquire);
        if limited && max > 0 && self.pending.load(Ordering::Acquire) >= max {
            self.lose(1, op.kind());
            return Pushed::Lost;
        }
        let mut tx = self.tx.lock().unwrap_or_else(|p| p.into_inner());
        let sender = tx.get_or_insert_with(|| self.start(db, live));
        self.pending.fetch_add(1, Ordering::AcqRel);
        if let Err(back) = sender.send(op) {
            // Поток-писатель завершился (паника) — новый, операция — в него.
            let s = self.start(db, live);
            if s.send(back.0).is_err() {
                self.done(1);
            }
            *tx = Some(s);
        }
        Pushed::Queued
    }

    /// Решение 354: новая метка повторяющейся строки (в памяти, без базы). Ошибок нет.
    pub(super) fn next_token(&self) -> u64 {
        self.token.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Предел записей в одной пачке (0 — всё накопленное). Ошибок нет.
    pub(super) fn batch_max(&self) -> usize {
        self.batch.load(Ordering::Acquire)
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
    pub(super) fn lose(&self, n: u64, kind: &str) {
        self.lost.fetch_add(n, Ordering::AcqRel);
        let total = self.lost_total.fetch_add(n, Ordering::AcqRel) + n;
        tracing::warn!(
            kind,
            total,
            "запись журнала потеряна (очередь journal.queue_max)"
        );
    }

    /// n записей ушли из очереди; пусто — сигнал ждущим. Ошибок нет.
    pub(super) fn done(&self, n: usize) {
        if self.pending.fetch_sub(n, Ordering::AcqRel) == n {
            self.idle.notify_waiters();
        }
    }

    /// Запись о потерях с прошлой (если были). Возвращает: запись или None. Ошибок нет.
    pub(super) fn lost_note(&self) -> Option<Entry> {
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
    fn start(self: &Arc<Self>, db: &Db, live: &broadcast::Sender<Entry>) -> Sender<Op> {
        let (tx, rx) = channel();
        let (db, live, me) = (db.clone(), live.clone(), Arc::downgrade(self));
        let r = std::thread::Builder::new()
            .name("journal-writer".into())
            .spawn(move || super::journal_writer::run(rx, db, live, me));
        if let Err(e) = r {
            tracing::error!(%e, "поток записи журнала не запущен");
        }
        tx
    }
}

/// Итог постановки операции.
pub(super) enum Pushed {
    /// Поставлена.
    Queued,
    /// Не поставлена: очередь полна (потеря посчитана).
    Lost,
    /// Очередь выключена — выполнить прямо.
    Off(Op),
}
