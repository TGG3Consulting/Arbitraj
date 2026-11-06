//! Файл: storage/uncapped_db.rs
//! Что делает: записи расчёта «без потолка» (решение 141) в SQLite — таблица uncapped: id, вид (deal — сделка,
//! candidate — кандидат-эпизод), id сделки, тикер, начало (эпизода или расчёта сделки), обновлено, JSON записи (тип —
//! core/uncapped_types.rs Record). Начало и вид — отдельными столбцами для выборки за период и уборки по сроку.
//! Кто вызывает: core/uncapped_store.rs (запись, обновление эпизода, уборка), core/uncapped_view.rs (выборки).
//! От чего зависит: storage/db.rs.
//! Настройки: нет (срок хранения uncapped.keep_days читает core/uncapped_store.rs).

use rusqlite::{OptionalExtension, params, params_from_iter, types::Value};

use super::db::{Db, DbError};

/// Новая строка.
pub struct Row {
    pub kind: &'static str,
    pub deal_id: Option<String>,
    pub ticker: String,
    pub started_at: String,
    pub body: String,
}

/// Выборка: период [from, to) по началу (RFC 3339), вид (None — все).
#[derive(Debug, Default, Clone)]
pub struct Filter {
    pub from: Option<String>,
    pub to: Option<String>,
    pub kind: Option<String>,
}

/// Добавляет строку. Возвращает: id. Ошибки: база.
pub async fn insert(db: &Db, r: Row) -> Result<i64, DbError> {
    db.call(move |c| {
        c.execute(
            "INSERT INTO uncapped (kind, deal_id, ticker, started_at, updated_at, body)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            params![r.kind, r.deal_id, r.ticker, r.started_at, r.body],
        )?;
        Ok(c.last_insert_rowid())
    })
    .await
}

/// Заменяет JSON строки (эпизод продолжился). Ошибки: база.
pub async fn update(db: &Db, id: i64, updated_at: String, body: String) -> Result<(), DbError> {
    db.call(move |c| {
        c.execute(
            "UPDATE uncapped SET updated_at = ?2, body = ?3 WHERE id = ?1",
            params![id, updated_at, body],
        )
        .map(|_| ())
    })
    .await
}

/// Удаляет строки, начатые раньше `before` (RFC 3339). Возвращает: сколько удалено. Ошибки: база.
pub async fn prune(db: &Db, before: String) -> Result<usize, DbError> {
    db.call(move |c| c.execute("DELETE FROM uncapped WHERE started_at < ?1", [before]))
        .await
}

/// Все строки выборки (id, JSON), новые сверху. Ошибки: база.
pub async fn all(db: &Db, f: Filter) -> Result<Vec<(i64, String)>, DbError> {
    db.call(move |c| {
        let (wh, args) = where_of(&f);
        let sql = format!("SELECT id, body FROM uncapped{wh} ORDER BY started_at DESC");
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(params_from_iter(args.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        rows.collect()
    })
    .await
}

/// Одна строка по id. Ошибки: база.
pub async fn get(db: &Db, id: i64) -> Result<Option<String>, DbError> {
    db.call(move |c| {
        c.query_row("SELECT body FROM uncapped WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()
    })
    .await
}

/// Последняя запись сделки (id сделки). Ошибки: база.
pub async fn by_deal(db: &Db, deal_id: String) -> Result<Option<(i64, String)>, DbError> {
    db.call(move |c| {
        c.query_row(
            "SELECT id, body FROM uncapped WHERE deal_id = ?1 ORDER BY id DESC LIMIT 1",
            [deal_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
    })
    .await
}

/// Последняя запись каждой сделки, у которой она есть: (id сделки, id записи, JSON) — одним запросом для экрана
/// «Сделки» (вместо запроса на каждую сделку). Ошибки: база.
pub async fn by_deals(db: &Db) -> Result<Vec<(String, i64, String)>, DbError> {
    db.call(|c| {
        let mut st = c.prepare(
            "SELECT u.deal_id, u.id, u.body FROM uncapped u JOIN \
             (SELECT MAX(id) AS m FROM uncapped WHERE deal_id IS NOT NULL GROUP BY deal_id) t ON u.id = t.m",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    })
    .await
}

/// Условие WHERE и параметры. Ошибок нет.
fn where_of(f: &Filter) -> (String, Vec<Value>) {
    let mut cond = vec![];
    let mut args = vec![];
    if let Some(v) = &f.from {
        cond.push("started_at >= ?");
        args.push(Value::Text(v.clone()));
    }
    if let Some(v) = &f.to {
        cond.push("started_at < ?");
        args.push(Value::Text(v.clone()));
    }
    if let Some(v) = &f.kind {
        cond.push("kind = ?");
        args.push(Value::Text(v.clone()));
    }
    if cond.is_empty() {
        return (String::new(), args);
    }
    (format!(" WHERE {}", cond.join(" AND ")), args)
}
