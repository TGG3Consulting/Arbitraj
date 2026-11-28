//! Курсор и изменения Raindex сохраняются одной транзакцией. Старый читатель не заменяет новый курсор.
use super::{
    history_orders::{Cursor, Delta},
    source::Raw,
    wire, *,
};
use crate::storage::{
    db::{Db, DbError},
    venue_state,
};
use rusqlite::{OptionalExtension, params};

pub(super) struct Stored {
    pub expected: Option<String>,
    pub valid: Option<(Cursor, Arc<Raw>)>,
}

pub(super) async fn load(ctx: &Ctx, namespace: &str, version: u8) -> Result<Stored, DbError> {
    let rows = venue_state::list(ctx.journal.db(), ctx.place.id, namespace, "").await?;
    let mut expected = None;
    let mut orders = Raw::new();
    let mut valid = true;
    for (key, body) in rows {
        if key == "cursor" {
            expected = Some(body);
            continue;
        }
        let pair = key.parse::<B256>().ok().zip(body.parse::<Bytes>().ok());
        if let Some((hash, bytes)) = pair
            && wire::Order::decode(version, &bytes, hash).is_some()
        {
            orders.insert(hash, bytes);
        } else {
            valid = false;
        }
    }
    let cursor = expected
        .as_deref()
        .and_then(|v| serde_json::from_str(v).ok());
    Ok(Stored {
        expected,
        valid: cursor.filter(|_| valid).map(|c| (c, Arc::new(orders))),
    })
}

pub(super) async fn commit(
    db: &Db,
    venue: &str,
    namespace: &str,
    previous: Option<String>,
    cursor: Cursor,
    reset: bool,
    mutations: Delta,
) -> Result<bool, DbError> {
    let (venue, namespace) = (venue.to_owned(), namespace.to_owned());
    db.call(move |c| {
        let tx = c.transaction()?;
        let found: Option<String> = tx
            .query_row(
                "SELECT body FROM venue_state WHERE venue=?1 AND kind=?2 AND id='cursor'",
                params![venue, namespace],
                |row| row.get(0),
            )
            .optional()?;
        if found != previous {
            return Ok(false);
        }
        if reset {
            tx.execute(
                "DELETE FROM venue_state WHERE venue=?1 AND kind=?2",
                params![venue, namespace],
            )?;
        }
        let at = chrono::Utc::now().to_rfc3339();
        {
            let mut write = tx.prepare(
                "INSERT INTO venue_state(venue,kind,id,body,updated_at)
                VALUES(?1,?2,?3,?4,?5) ON CONFLICT(venue,kind,id) DO UPDATE SET
                body=excluded.body,updated_at=excluded.updated_at",
            )?;
            let mut remove =
                tx.prepare("DELETE FROM venue_state WHERE venue=?1 AND kind=?2 AND id=?3")?;
            for (hash, data) in mutations {
                let key = format!("{hash:#x}");
                if let Some(bytes) = data {
                    write.execute(params![venue, namespace, key, bytes.to_string(), at])?;
                } else {
                    remove.execute(params![venue, namespace, key])?;
                }
            }
            write.execute(params![
                venue,
                namespace,
                "cursor",
                serde_json::json!(cursor).to_string(),
                at
            ])?;
        }
        tx.commit()?;
        Ok(true)
    })
    .await
}
