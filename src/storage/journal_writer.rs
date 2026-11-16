//! Файл: storage/journal_writer.rs
//! Что делает: решение 341 — поток-писатель очереди журнала (вынесен из journal_queue.rs): ждёт первую операцию,
//! забирает накопленное (не больше journal.batch_max), выполняет одной транзакцией по порядку постановки, затем отдаёт
//! записи в живой поток интерфейса с настоящими id. Решение 354: держит карту «метка повторяющейся строки → id» —
//! обновления по метке идут после вставки (один поток, порядок постановки); пачка не записалась — метки этой пачки
//! из карты убираются (их обновления пропускаются; потеря посчитана).
//! Кто вызывает: storage/journal_queue.rs (Queue::start).
//! От чего зависит: storage/db.rs (call_sync), storage/journal_op.rs (Op, apply), storage/journal_queue.rs (Queue).
//! Настройки: journal.batch_max (через Queue::batch_max).

use std::collections::HashMap;
use std::sync::Weak;
use std::sync::mpsc::Receiver;

use tokio::sync::broadcast;

use super::db::Db;
use super::journal::Entry;
use super::journal_op::{Op, apply};
use super::journal_queue::Queue;

/// Поток-писатель. Завершается, когда очередь (все клоны Journal) удалена. Принимает: приёмник, базу, живой поток,
/// очередь. Ошибок нет.
pub(super) fn run(rx: Receiver<Op>, db: Db, live: broadcast::Sender<Entry>, q: Weak<Queue>) {
    let mut ids: HashMap<u64, i64> = HashMap::new();
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        let cap = q.upgrade().map_or(0, |q| q.batch_max());
        while cap == 0 || batch.len() < cap {
            let Ok(op) = rx.try_recv() else {
                break;
            };
            batch.push(op);
        }
        let taken = batch.len();
        if let Some(note) = q.upgrade().and_then(|q| q.lost_note()) {
            batch.push(Op::Insert(Box::new(note)));
        }
        let mut out: Vec<Entry> = Vec::with_capacity(batch.len());
        let mut fresh: Vec<u64> = Vec::new();
        let res = db.call_sync(|c| {
            let t = c.transaction()?;
            for op in batch {
                apply(&t, op, &mut ids, &mut out, &mut fresh)?;
            }
            t.commit()
        });
        let q = q.upgrade();
        match res {
            Ok(()) => {
                for e in out {
                    // Нет слушателей — не ошибка: интерфейс может быть закрыт.
                    let _ = live.send(e);
                }
            }
            Err(err) => {
                tracing::error!(%err, n = taken, "пачка журнала не записана");
                for t in fresh {
                    ids.remove(&t);
                }
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
