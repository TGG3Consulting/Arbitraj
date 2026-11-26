//! Очередь дежурного в основной базе. Писатель торгового учёта и старое зеркало не участвуют.
use super::incident::Incident;
use crate::storage::db::{Db, DbError};
use std::collections::BTreeMap;

pub async fn put(db: &Db, items: &BTreeMap<String, Incident>) -> Result<(), DbError> {
    let rows: Vec<_> = items
        .iter()
        .map(|(key, i)| serde_json::to_string(i).map(|body| (key.clone(), body)))
        .collect::<Result<_, _>>()
        .map_err(|e| DbError::Json(e.to_string()))?;
    db.call(move |c| {
        let tx = c.transaction()?;
        for (key, body) in rows {
            tx.execute(
                "INSERT OR IGNORE INTO notify_duty_pending(incident,body) VALUES (?1,?2)",
                [key, body],
            )?;
        }
        tx.commit()
    })
    .await
}

pub async fn load(db: &Db) -> Result<BTreeMap<String, Incident>, DbError> {
    let rows: Vec<(String, String)> = db
        .call(|c| {
            let mut stmt = c.prepare("SELECT incident, body FROM notify_duty_pending")?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .await?;
    rows.into_iter()
        .map(|(k, body)| serde_json::from_str(&body).map(|i| (k, i)))
        .collect::<Result<_, _>>()
        .map_err(|e| DbError::Json(e.to_string()))
}

pub async fn remove(db: &Db, done: Vec<String>) -> Result<(), DbError> {
    if done.is_empty() {
        return Ok(());
    }
    db.call(move |c| {
        let tx = c.transaction()?;
        for key in done {
            tx.execute("DELETE FROM notify_duty_pending WHERE incident=?1", [key])?;
        }
        tx.commit()
    })
    .await
}
