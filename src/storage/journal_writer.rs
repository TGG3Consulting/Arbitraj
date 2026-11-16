//! Файл: storage/journal_writer.rs
//! Что делает: решение 341 — поток-писатель очереди журнала (storage/journal_queue.rs): ждёт первую операцию, забирает
//! накопленное (не больше journal.batch_max), выполняет одной транзакцией по порядку постановки под замком основной
//! базы, затем отдаёт записи в их живые потоки с настоящими id. Решение 354: держит карту «метка повторяющейся строки →
//! id» — обновления по метке идут после вставки (один поток, порядок постановки).
//! Ревью t158: пачка не записалась (Н2) — повтор той же пачки по одной операции в своей транзакции, по порядку:
//! теряются только операции, которые не записались сами (счёт потерь и запись journal.queue_lost); запись о потерях
//! не записалась (Н5) — её счёт возвращается. Паника при записи (Н3) — пачка считается потерянной, но счётчики
//! «записано» продвигаются (ожидающие чтения и остановка не зависают), поток живёт дальше.
//! Завершается, когда очередь закрыта (база закрывается — Queue::shutdown) и всё полученное дописано.
//! Кто вызывает: storage/journal_queue.rs (Queue::start).
//! От чего зависит: storage/journal_op.rs (Op, Row, apply), storage/journal_queue.rs (Queue), rusqlite.
//! Настройки: journal.batch_max (через Queue::batch_max).

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, Weak};

use rusqlite::Connection;

use super::journal::Entry;
use super::journal_op::{Live, Op, Row, apply};
use super::journal_queue::Queue;

/// Карта «метка → id» и живой поток для записи о потерях (последний встреченный).
#[derive(Default)]
struct Mem {
    ids: HashMap<u64, i64>,
    last_live: Option<Live>,
}

/// Поток-писатель. Принимает: приёмник, соединение основной базы, очередь. Ошибок нет.
pub(super) fn run(rx: Receiver<Op>, conn: Arc<Mutex<Connection>>, q: Weak<Queue>) {
    let mut mem = Mem::default();
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
        let lost = match catch_unwind(AssertUnwindSafe(|| one_batch(&conn, &q, batch, &mut mem))) {
            Ok(n) => n,
            Err(_) => {
                tracing::error!(
                    n = taken,
                    "паника при записи пачки журнала — пачка потеряна"
                );
                taken
            }
        };
        if let Some(q) = q.upgrade() {
            if lost > 0 {
                q.lose(lost as u64, "пачка");
            }
            q.done(taken);
        }
    }
}

/// Одна пачка: целиком одной транзакцией, при ошибке — по одной операции. Принимает: соединение, очередь, операции,
/// память писателя. Возвращает: сколько операций очереди потеряно (запись о потерях не считается). Ошибок нет.
fn one_batch(
    conn: &Mutex<Connection>,
    q: &Weak<Queue>,
    mut batch: Vec<Op>,
    mem: &mut Mem,
) -> usize {
    if let Some(l) = batch.iter().rev().find_map(Op::live) {
        mem.last_live = Some(l.clone());
    }
    let ops = batch.len();
    let note = q.upgrade().and_then(|q| q.lost_note());
    let since = note.as_ref().map_or(0, |(_, n)| *n);
    if let Some((e, _)) = note {
        let live = mem.last_live.clone();
        batch.push(Op::Insert(Box::new(Row { e, live })));
    }
    // «Забыть метку» в неудачной пачке тоже откатывается: карта меток — как до пачки (копия — только если есть что
    // забывать).
    let backup = batch
        .iter()
        .any(|o| matches!(o, Op::Forget(_)))
        .then(|| mem.ids.clone());
    let mut out = Vec::with_capacity(batch.len());
    let mut fresh = Vec::new();
    if let Err(err) = write(conn, &batch, &mut mem.ids, &mut out, &mut fresh) {
        tracing::error!(%err, n = batch.len(), "пачка журнала не записана — повтор по одной записи");
        match backup {
            Some(b) => mem.ids = b,
            None => {
                for t in fresh {
                    mem.ids.remove(&t);
                }
            }
        }
        return by_one(conn, q, &batch, ops, since, mem);
    }
    send(out);
    0
}

/// Повтор пачки по одной операции, по порядку. Принимает: соединение, очередь, операции (последняя — запись о потерях,
/// если since > 0), число операций очереди, счёт записи о потерях, память. Возвращает: сколько операций очереди не
/// записалось. Ошибок нет.
fn by_one(
    conn: &Mutex<Connection>,
    q: &Weak<Queue>,
    batch: &[Op],
    ops: usize,
    since: u64,
    mem: &mut Mem,
) -> usize {
    let mut lost = 0;
    for (i, op) in batch.iter().enumerate() {
        let mut out = Vec::with_capacity(1);
        let mut fresh = Vec::new();
        match write(
            conn,
            std::slice::from_ref(op),
            &mut mem.ids,
            &mut out,
            &mut fresh,
        ) {
            Ok(()) => send(out),
            Err(err) => {
                tracing::error!(%err, kind = op.kind(), "запись журнала не записана");
                for t in fresh {
                    mem.ids.remove(&t);
                }
                if i < ops {
                    lost += 1;
                } else if let Some(q) = q.upgrade() {
                    q.unlose(since);
                }
            }
        }
    }
    lost
}

/// Записанное — в живые потоки. Нет слушателей — не ошибка: интерфейс может быть закрыт. Ошибок нет.
fn send(out: Vec<(Entry, Option<Live>)>) {
    for (e, live) in out {
        if let Some(l) = live {
            let _ = l.send(e);
        }
    }
}

/// Операции — одной транзакцией под замком основной базы. Принимает: соединение, операции, карту меток, выход для
/// живых потоков, новые метки. Ошибки: SQLite (текстом).
fn write(
    conn: &Mutex<Connection>,
    ops: &[Op],
    ids: &mut HashMap<u64, i64>,
    out: &mut Vec<(Entry, Option<Live>)>,
    fresh: &mut Vec<u64>,
) -> Result<(), String> {
    // Замок отравлен (паника у другого пользователя базы) — журнал продолжает писать: соединение SQLite целое.
    let mut c = conn.lock().unwrap_or_else(|p| p.into_inner());
    let t = c.transaction().map_err(|e| e.to_string())?;
    for op in ops {
        apply(&t, op, ids, out, fresh).map_err(|e| e.to_string())?;
    }
    t.commit().map_err(|e| e.to_string())
}
