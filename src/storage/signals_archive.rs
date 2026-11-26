//! Ограничивает выборку сводок для суточных «Сигналов и сделок». SQLite читает отдельное WAL-соединение архива.
//! Точные границы строк книги проверяет sig_build с исходной точностью chrono. Здесь сравнение включительное,
//! чтобы округление julianday до миллисекунд не исключило запись на границе.

use chrono::{DateTime, Utc};

use super::db::{Db, DbError};

/// Окна, пересекающие период, и явные привязки сделок. Последние имеют приоритет перед совпадением времени.
pub async fn windows(
    db: &Db,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> Result<Vec<String>, DbError> {
    db.call(move |c| {
        let mut st = c.prepare(
            "SELECT body FROM windows WHERE deal OR
            ((?2 IS NULL OR julianday(started_at) <= julianday(?2)) AND
             (?1 IS NULL OR CASE WHEN json_valid(body) THEN
               coalesce(julianday(json_extract(body, '$.ended_at')) >= julianday(?1), 1) ELSE 0 END))
             ORDER BY started_at ASC")?;
        let rows = st.query_map(rusqlite::params![from.map(|v| v.to_rfc3339()), to.map(|v| v.to_rfc3339())], |r| r.get(0))?;
        rows.collect()
    }).await
}

/// Эпизоды между началом первого выводимого окна и концом последнего, включая начавшиеся раньше первого окна.
pub async fn episodes(
    db: &Db,
    first: DateTime<Utc>,
    last: DateTime<Utc>,
) -> Result<Vec<String>, DbError> {
    db.call(move |c| {
        let mut st = c.prepare(
            "SELECT body FROM episodes WHERE julianday(started_at) <= julianday(?2) AND
             CASE WHEN json_valid(body) THEN
               coalesce(julianday(json_extract(body, '$.ended_at')) >= julianday(?1), 1) ELSE 0 END
             ORDER BY started_at ASC",
        )?;
        let rows = st.query_map(
            rusqlite::params![first.to_rfc3339(), last.to_rfc3339()],
            |r| r.get(0),
        )?;
        rows.collect()
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{episodes_db, windows_db};
    use serde_json::json;

    /// Границы наблюдений — первые и последние кадры трёх настоящих окон book_frames.jsonl.gz
    /// (tests/data/sigma_windows_2026-11-16). Проверяется именно выборка SQLite по временам и флагу привязки.
    #[tokio::test]
    async fn separate_reader_preserves_crossing_intervals_and_explicit_links() {
        let dir = std::env::temp_dir().join(format!("arb-signals-reader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("archive.sqlite");
        let db = Db::open(&path).unwrap();
        let intervals = [
            (
                "20261114221311257-AVAX",
                "2026-11-14T22:13:11.745225Z",
                "2026-11-14T22:13:28.007218Z",
                true,
            ),
            (
                "20261114221621492-4STOCK",
                "2026-11-14T22:16:21.603016Z",
                "2026-11-14T22:17:06.601972Z",
                false,
            ),
            (
                "20261115003157603-CELL",
                "2026-11-15T00:31:57.863858Z",
                "2026-11-15T00:32:32.363526Z",
                false,
            ),
        ];
        let mut bodies = Vec::new();
        for (id, start, end, linked) in intervals {
            let body = json!({"id":id, "started_at":start, "ended_at":end}).to_string();
            windows_db::save(
                &db,
                windows_db::Row {
                    id: id.into(),
                    started_at: start.into(),
                    ticker: id.split_once('-').unwrap().1.into(),
                    address: None,
                    deal: linked,
                    outcome: None,
                    body: body.clone(),
                },
            )
            .await
            .unwrap();
            episodes_db::save(&db, id, start.into(), body.clone())
                .await
                .unwrap();
            bodies.push(body);
        }
        let reader = db.report_reader().await.unwrap();
        assert!(
            reader
                .call_sync(|c| c.execute_batch("DELETE FROM windows"))
                .is_err()
        );
        assert!(
            reader
                .ledger()
                .write(|c| c.execute_batch("DELETE FROM core_state"))
                .await
                .is_err()
        );
        let edge = intervals[1].2.parse::<DateTime<Utc>>().unwrap();
        assert_eq!(
            windows(&reader, Some(edge), Some(edge)).await.unwrap(),
            bodies[..2]
        );
        assert_eq!(episodes(&reader, edge, edge).await.unwrap(), bodies[1..2]);
        drop(reader);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
