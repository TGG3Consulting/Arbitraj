//! Файл: storage/journal_op.rs
//! Что делает: решение 354 (задача t158) — операции очереди записи журнала и их выполнение потоком-писателем
//! (storage/journal_queue.rs): обычная запись; «повторяющаяся» строка (решение 155, счётчик повторов) с номером-меткой,
//! выданным в памяти; её обновление по метке или по настоящему id; «забыть» метку. Метка нужна, чтобы строка отказов
//! и её обновления не ждали основную базу ради id: id строки знает только писатель (карта метка → id), обновления идут
//! той же очередью после вставки — порядок «строка → обновления» держит сама очередь (один поток, по порядку).
//! Кто вызывает: storage/journal_queue.rs (Op, apply), storage/journal_counted.rs (постановка).
//! От чего зависит: storage/journal.rs (Entry, insert, COLUMNS, row_entry), rusqlite.
//! Настройки: нет.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use super::journal::{COLUMNS, Entry, insert, row_entry};

/// Какую строку обновить: по метке (строка вставлена очередью) или по настоящему id (вставлена прямо).
#[derive(Debug, Clone, Copy)]
pub enum Target {
    Token(u64),
    Id(i64),
}

/// Обновление повторяющейся строки: счётчик, время последнего, текст и данные — последние.
pub struct Upd {
    pub target: Target,
    pub repeats: i64,
    pub last_at: DateTime<Utc>,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// Операция очереди журнала (большие части — в куче: размер операции — пара слов).
pub enum Op {
    /// Обычная запись.
    Insert(Box<Entry>),
    /// Повторяющаяся строка (счётчик 1) с меткой.
    Counted(u64, Box<Entry>),
    /// Обновление повторяющейся строки.
    Update(Box<Upd>),
    /// Метка больше не нужна (строка закрыта).
    Forget(u64),
}

impl Op {
    /// Вид записи для лога потерь. Ошибок нет.
    pub fn kind(&self) -> &str {
        match self {
            Op::Insert(e) | Op::Counted(_, e) => &e.kind,
            Op::Update(_) => "обновление строки повторов",
            Op::Forget(_) => "метка строки повторов",
        }
    }
}

/// Что делает: выполняет операцию в транзакции пачки. Принимает: соединение (транзакцию), операцию, карту меток,
/// записи для живого потока (дописываются), метки, вставленные этой пачкой (дописываются — при откате пачки их
/// убирают из карты). Возвращает: ничего. Ошибки: SQLite (пачка откатывается целиком).
pub fn apply(
    c: &Connection,
    op: Op,
    ids: &mut HashMap<u64, i64>,
    live: &mut Vec<Entry>,
    fresh: &mut Vec<u64>,
) -> rusqlite::Result<()> {
    match op {
        Op::Insert(mut e) => {
            e.id = insert(c, &e)?;
            live.push(*e);
        }
        Op::Counted(token, mut e) => {
            e.id = insert_counted(c, &e)?;
            ids.insert(token, e.id);
            fresh.push(token);
            live.push(*e);
        }
        Op::Update(u) => {
            let id = match u.target {
                Target::Id(id) => Some(id),
                Target::Token(t) => ids.get(&t).copied(),
            };
            // Метки нет — строка не записалась (пачка с ней не прошла): обновлять нечего (потеря уже посчитана).
            if let Some(e) = id.map(|id| update(c, id, &u)).transpose()?.flatten() {
                live.push(e);
            }
        }
        Op::Forget(token) => {
            ids.remove(&token);
        }
    }
    Ok(())
}

/// Вставляет повторяющуюся строку (счётчик 1, время последнего = время записи; как Journal::write_counted).
/// Принимает: соединение, запись. Возвращает: id. Ошибки: SQLite.
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

/// Обновляет повторяющуюся строку (как Journal::update_counted: счётчик только растёт). Принимает: соединение, id,
/// обновление. Возвращает: запись после обновления (None — записи нет или обновление опоздало). Ошибки: SQLite.
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
