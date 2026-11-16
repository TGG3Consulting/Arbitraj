//! Файл: storage/journal_writer.rs
//! Что делает: решение 341 — поток-писатель очереди журнала (storage/journal_queue.rs): ждёт первую операцию, забирает
//! накопленное (не больше journal.batch_max), выполняет одной транзакцией по порядку постановки под замком основной
//! базы, затем отдаёт записи в их живые потоки с настоящими id. Решение 354: держит карту «метка повторяющейся строки →
//! id» — обновления по метке идут после вставки (один поток, порядок постановки); пачка не записалась — метки этой
//! пачки из карты убираются (их обновления пропускаются; потеря посчитана). Завершается, когда очередь закрыта
//! (база закрывается — Queue::shutdown) и всё полученное дописано.
//! Кто вызывает: storage/journal_queue.rs (Queue::start).
//! От чего зависит: storage/journal_op.rs (Op, Row, apply), storage/journal_queue.rs (Queue), rusqlite.
//! Настройки: journal.batch_max (через Queue::batch_max).

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, Weak};

use rusqlite::Connection;

use super::journal::Entry;
use super::journal_op::{Live, Op, Row, apply};
use super::journal_queue::Queue;

/// Поток-писатель. Принимает: приёмник, соединение основной базы, очередь. Ошибок нет.
pub(super) fn run(rx: Receiver<Op>, conn: Arc<Mutex<Connection>>, q: Weak<Queue>) {
    let mut ids: HashMap<u64, i64> = HashMap::new();
    // Живой поток для записи о потерях — последний встреченный (журнал основной базы обычно один).
    let mut last_live: Option<Live> = None;
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
        if let Some(l) = batch.iter().rev().find_map(Op::live) {
            last_live = Some(l.clone());
        }
        if let Some(note) = q.upgrade().and_then(|q| q.lost_note()) {
            let live = last_live.clone();
            batch.push(Op::Insert(Box::new(Row { e: note, live })));
        }
        let mut out: Vec<(Entry, Option<Live>)> = Vec::with_capacity(batch.len());
        let mut fresh: Vec<u64> = Vec::new();
        let res = write(&conn, batch, &mut ids, &mut out, &mut fresh);
        let q = q.upgrade();
        match res {
            Ok(()) => {
                for (e, live) in out {
                    // Нет слушателей — не ошибка: интерфейс может быть закрыт.
                    if let Some(l) = live {
                        let _ = l.send(e);
                    }
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

/// Одна пачка — одна транзакция под замком основной базы. Принимает: соединение, операции, карту меток, выход для
/// живых потоков, новые метки пачки. Ошибки: SQLite, замок отравлен.
fn write(
    conn: &Mutex<Connection>,
    batch: Vec<Op>,
    ids: &mut HashMap<u64, i64>,
    out: &mut Vec<(Entry, Option<Live>)>,
    fresh: &mut Vec<u64>,
) -> Result<(), String> {
    let mut c = conn
        .lock()
        .map_err(|_| "замок основной базы отравлен".to_string())?;
    let t = c.transaction().map_err(|e| e.to_string())?;
    for op in batch {
        apply(&t, op, ids, out, fresh).map_err(|e| e.to_string())?;
    }
    t.commit().map_err(|e| e.to_string())
}
