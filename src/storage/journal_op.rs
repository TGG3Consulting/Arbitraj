//! Файл: storage/journal_op.rs
//! Что делает: решения 341, 354 (задача t158) — операции очереди записи журнала и их выполнение потоком-писателем
//! (storage/journal_writer.rs): обычная запись; «повторяющаяся» строка (решение 155, счётчик повторов) с меткой,
//! выданной в памяти; её обновление по метке; «забыть» метку. Метка нужна, чтобы строка отказов и её обновления не
//! ждали основную базу ради id: id строки знает только писатель (карта метка → id), обновления идут той же очередью
//! после вставки — порядок «строка → обновления» держит сама очередь (один поток, по порядку постановки). У записи
//! и обновления — свой живой поток (того журнала, который их поставил): после записи пачки запись с настоящим id
//! уходит туда.
//! Кто вызывает: storage/journal.rs, storage/journal_counted.rs (постановка), storage/journal_writer.rs (apply).
//! От чего зависит: storage/journal.rs (Entry, insert, COLUMNS, row_entry), rusqlite, tokio::sync::broadcast.
//! Настройки: нет.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use tokio::sync::broadcast;

use super::journal::{COLUMNS, Entry, insert, row_entry};

/// Живой поток журнала (интерфейс).
pub type Live = broadcast::Sender<Entry>;

/// Запись и живой поток, куда отдать её после записи (None — никуда: запись о потерях без журнала-источника).
#[derive(Clone)]
pub struct Row {
    pub e: Entry,
    pub live: Option<Live>,
}

/// Обновление повторяющейся строки по метке: счётчик, время последнего, текст и данные — последние.
#[derive(Clone)]
pub struct Upd {
    pub token: u64,
    pub repeats: i64,
    pub last_at: DateTime<Utc>,
    pub message: String,
    pub data: Option<serde_json::Value>,
    pub live: Live,
}

/// Операция очереди журнала (большие части — в куче: размер операции — пара слов). Клонируется — для повтора по
/// одной после ошибки пачки (journal_writer.rs).
#[derive(Clone)]
pub enum Op {
    /// Обычная запись.
    Insert(Box<Row>),
    /// Повторяющаяся строка (счётчик 1) с меткой.
    Counted(u64, Box<Row>),
    /// Обновление повторяющейся строки.
    Update(Box<Upd>),
    /// Метка больше не нужна (строка закрыта).
    Forget(u64),
}

impl Op {
    /// Вид записи для лога потерь. Ошибок нет.
    pub fn kind(&self) -> &str {
        match self {
            Op::Insert(r) | Op::Counted(_, r) => &r.e.kind,
            Op::Update(_) => "обновление строки повторов",
            Op::Forget(_) => "метка строки повторов",
        }
    }

    /// Живой поток операции (если есть). Ошибок нет.
    pub fn live(&self) -> Option<&Live> {
        match self {
            Op::Insert(r) | Op::Counted(_, r) => r.live.as_ref(),
            Op::Update(u) => Some(&u.live),
            Op::Forget(_) => None,
        }
    }
}

/// Что делает: выполняет операцию в транзакции пачки. Принимает: соединение (транзакцию), операцию, карту меток,
/// записи для живых потоков (дописываются), метки, вставленные этой пачкой (дописываются — при откате пачки их
/// убирают из карты). Возвращает: ничего. Ошибки: SQLite (пачка откатывается целиком).
pub fn apply(
    c: &Connection,
    op: &Op,
    ids: &mut HashMap<u64, i64>,
    out: &mut Vec<(Entry, Option<Live>)>,
    fresh: &mut Vec<u64>,
) -> rusqlite::Result<()> {
    match op {
        Op::Insert(r) => {
            let mut e = r.e.clone();
            e.id = insert(c, &e)?;
            out.push((e, r.live.clone()));
        }
        Op::Counted(token, r) => {
            let mut e = r.e.clone();
            e.id = insert_counted(c, &e)?;
            ids.insert(*token, e.id);
            fresh.push(*token);
            out.push((e, r.live.clone()));
        }
        Op::Update(u) => {
            // Метки нет — строка не записалась (пачка с ней не прошла): обновлять нечего (потеря уже посчитана).
            if let Some(&id) = ids.get(&u.token)
                && let Some(e) = update(c, id, u)?
            {
                out.push((e, Some(u.live.clone())));
            }
        }
        Op::Forget(token) => {
            ids.remove(token);
        }
    }
    Ok(())
}

/// Вставляет повторяющуюся строку (счётчик 1, время последнего = время записи). Принимает: соединение, запись.
/// Возвращает: id. Ошибки: SQLite.
fn insert_counted(c: &Connection, e: &Entry) -> rusqlite::Result<i64> {
    c.execute(
        "INSERT INTO journal (at, level, kind, venue, message, data, repeats, last_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?1)",
        params![
            e.at.to_rfc3339(),
            format!("{:?}", e.level),
            e.kind,
            e.venue,
            e.message,
            e.data.as_ref().map(|d| d.to_string())
        ],
    )?;
    Ok(c.last_insert_rowid())
}

/// Обновляет повторяющуюся строку (счётчик только растёт: опоздавшее обновление ничего не меняет). Принимает:
/// соединение, id, обновление. Возвращает: запись после обновления (None — записи нет или обновление опоздало).
/// Ошибки: SQLite.
fn update(c: &Connection, id: i64, u: &Upd) -> rusqlite::Result<Option<Entry>> {
    let n = c.execute(
        "UPDATE journal SET repeats = ?2, last_at = ?3, message = ?4, data = ?5
         WHERE id = ?1 AND (repeats IS NULL OR repeats <= ?2)",
        params![
            id,
            u.repeats,
            u.last_at.to_rfc3339(),
            u.message,
            u.data.as_ref().map(|d| d.to_string())
        ],
    )?;
    if n == 0 {
        return Ok(None);
    }
    c.query_row(
        &format!("SELECT {COLUMNS} FROM journal WHERE id = ?1"),
        params![id],
        row_entry,
    )
    .optional()
}
