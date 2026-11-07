//! Файл: storage/windows_db.rs
//! Что делает: список окон (решение 125) в SQLite — таблица windows: id, начало, тикер, была ли сделка, итог сделок
//! («plus» / «minus» / «zero», пусто — сделки не было или итога нет), JSON сводки (тип — core/window.rs). Тикер,
//! сделка и итог — отдельными столбцами для фильтров экрана «Окна». Сами кадры — в файле окна. Решение 211: адрес
//! контракта монеты — столбец address (0x… в нижнем регистре; у старых строк NULL — берётся из JSON сводки).
//! Кто вызывает: core/window_worker.rs (сохранение), core/window_cfg.rs (уборка, закрытие прерванных),
//! core/window_view.rs (список с фильтрами, одно окно).
//! От чего зависит: storage/db.rs.
//! Настройки: нет.

use chrono::Utc;
use rusqlite::{OptionalExtension, params_from_iter, types::Value};

use super::db::{Db, DbError};

/// Строка для сохранения.
pub struct Row {
    pub id: String,
    pub started_at: String,
    pub ticker: String,
    /// Решение 211: адрес контракта монеты (0x… в нижнем регистре); у монеты без адреса — None.
    pub address: Option<String>,
    pub deal: bool,
    pub outcome: Option<String>,
    pub body: String,
}

/// Фильтры списка (None — не фильтровать).
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// Тикер монеты (точное совпадение).
    pub ticker: Option<String>,
    /// Решение 211: адрес контракта монеты (нижний регистр, точное совпадение; у старых строк — из JSON сводки).
    pub address: Option<String>,
    /// true — только со сделкой, false — только без.
    pub deal: Option<bool>,
    /// «plus» / «minus» / «zero».
    pub outcome: Option<String>,
}

/// Сохраняет сводку (заменяет прежнюю). Принимает: строку. Ошибки: база.
pub async fn save(db: &Db, r: Row) -> Result<(), DbError> {
    let at = Utc::now().to_rfc3339();
    db.call(move |c| {
        c.execute(
            "INSERT INTO windows (id, started_at, ticker, deal, outcome, body, updated_at, address)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET deal = excluded.deal, outcome = excluded.outcome,
             body = excluded.body, updated_at = excluded.updated_at, address = excluded.address",
            rusqlite::params![
                r.id,
                r.started_at,
                r.ticker,
                r.deal,
                r.outcome,
                r.body,
                at,
                r.address
            ],
        )
        .map(|_| ())
    })
    .await
}

/// Что делает: страница списка, новые сверху. Принимает: фильтры, сколько, сколько пропустить.
/// Возвращает: (JSON сводок, всего по фильтрам). Ошибки: база.
pub async fn page(
    db: &Db,
    f: Filter,
    limit: u32,
    offset: u32,
) -> Result<(Vec<String>, u64), DbError> {
    db.call(move |c| {
        let (wh, mut args) = where_of(&f);
        let total: i64 = c.query_row(
            &format!("SELECT COUNT(*) FROM windows{wh}"),
            params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        args.push(Value::Integer(limit.into()));
        args.push(Value::Integer(offset.into()));
        let sql = format!("SELECT body FROM windows{wh} ORDER BY started_at DESC LIMIT ? OFFSET ?");
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(params_from_iter(args.iter()), |r| r.get(0))?;
        Ok((
            rows.collect::<Result<Vec<String>, _>>()?,
            total.max(0) as u64,
        ))
    })
    .await
}

/// Условие WHERE и его параметры. Ошибок нет.
fn where_of(f: &Filter) -> (String, Vec<Value>) {
    let mut cond = vec![];
    let mut args = vec![];
    if let Some(t) = &f.ticker {
        cond.push("ticker = ?");
        args.push(Value::Text(t.clone()));
    }
    if let Some(a) = &f.address {
        cond.push("lower(coalesce(address, json_extract(body, '$.asset.bsc_contract'))) = ?");
        args.push(Value::Text(a.clone()));
    }
    if let Some(d) = f.deal {
        cond.push("deal = ?");
        args.push(Value::Integer(d.into()));
    }
    if let Some(o) = &f.outcome {
        cond.push("outcome = ?");
        args.push(Value::Text(o.clone()));
    }
    if cond.is_empty() {
        return (String::new(), args);
    }
    (format!(" WHERE {}", cond.join(" AND ")), args)
}

/// Все сводки, старые сверху (уборка по сроку и месту). Ошибки: база.
pub async fn oldest_first(db: &Db) -> Result<Vec<String>, DbError> {
    db.call(|c| {
        let mut st = c.prepare("SELECT body FROM windows ORDER BY started_at ASC")?;
        let rows = st.query_map([], |r| r.get(0))?;
        rows.collect()
    })
    .await
}

/// Одна сводка. None — нет такого окна. Ошибки: база.
pub async fn get(db: &Db, id: &str) -> Result<Option<String>, DbError> {
    let id = id.to_string();
    db.call(move |c| {
        c.query_row("SELECT body FROM windows WHERE id = ?1", [id], |r| r.get(0))
            .optional()
    })
    .await
}

/// Удаляет сводку. Ошибки: база.
pub async fn delete(db: &Db, id: &str) -> Result<(), DbError> {
    let id = id.to_string();
    db.call(move |c| {
        c.execute("DELETE FROM windows WHERE id = ?1", [id])
            .map(|_| ())
    })
    .await
}
