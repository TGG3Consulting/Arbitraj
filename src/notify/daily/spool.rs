//! Отдельный файл SQLite. XLSX и решения об отправке не занимают соединения торговли.
//! Хранится одна серия. Законченный набор частей неизменен до следующего срока.

use crate::notify::tg_methods::DOCUMENT_MAX_BYTES;
use crate::report::sig_parts::File;
use chrono::{DateTime, NaiveTime, TimeDelta, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub(super) struct Spool(Arc<Mutex<Connection>>);

impl Spool {
    pub(super) async fn open(path: PathBuf) -> Result<Self, String> {
        tokio::task::spawn_blocking(move || {
            let db = Connection::open(path)?;
            // Единственный сборщик, в том числе при ошибочном старте второго процесса на том же каталоге.
            db.execute_batch("PRAGMA locking_mode=EXCLUSIVE; PRAGMA auto_vacuum=FULL;
                CREATE TABLE IF NOT EXISTS schedule(slot INTEGER NOT NULL, started INTEGER NOT NULL);
                CREATE TABLE IF NOT EXISTS audience(chat TEXT PRIMARY KEY);
                CREATE TABLE IF NOT EXISTS books(report TEXT PRIMARY KEY, attempts INTEGER NOT NULL,
                    next_try INTEGER NOT NULL, ready INTEGER NOT NULL);
                CREATE TABLE IF NOT EXISTS files(report TEXT NOT NULL, part INTEGER NOT NULL,
                    name TEXT NOT NULL, caption TEXT NOT NULL, data BLOB NOT NULL, PRIMARY KEY(report,part));
                CREATE TABLE IF NOT EXISTS delivery(chat TEXT NOT NULL, report TEXT NOT NULL, part INTEGER NOT NULL,
                    attempts INTEGER NOT NULL, status TEXT NOT NULL, retry INTEGER NOT NULL,
                    PRIMARY KEY(chat,report,part));")?;
            Ok::<_, rusqlite::Error>(Self(Arc::new(Mutex::new(db))))
        }).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
    }

    async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T, String> {
        let db = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut c = db
                .lock()
                .map_err(|_| "блокировка базы рассылки повреждена".to_string())?;
            f(&mut c).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// После первой настройки ждём ближайшее HH:MM. При восстановлении продолжаем последний
    /// наступивший срок, включая переход через полночь. Одна серия в дату даже при смене HH:MM.
    #[cfg(test)]
    pub(super) async fn slot(
        &self,
        now: DateTime<Utc>,
        time: NaiveTime,
    ) -> Result<Option<DateTime<Utc>>, String> {
        self.slot_for(now, time, vec![]).await
    }

    pub(super) async fn slot_for(
        &self,
        now: DateTime<Utc>,
        time: NaiveTime,
        chats: Vec<String>,
    ) -> Result<Option<DateTime<Utc>>, String> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let old: Option<(i64, bool)> = tx
                .query_row("SELECT slot,started FROM schedule", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            let old = old.and_then(|(s, started)| Some((DateTime::from_timestamp(s, 0)?, started)));
            let today = now.date_naive().and_time(time).and_utc();
            let latest = if now >= today {
                today
            } else {
                today - TimeDelta::days(1)
            };
            let (slot, started) = match old {
                Some((prev, true)) if prev.date_naive() >= latest.date_naive() => {
                    return Ok((prev <= now).then_some(prev));
                }
                Some((_, true)) => (latest, true),
                Some((prev, false)) if prev.time() == time && prev > now => return Ok(None),
                Some((prev, false)) if prev.time() == time => (latest, true),
                _ if now >= today && now < today + TimeDelta::minutes(1) => (today, true),
                _ if now < today => (today, false),
                _ => (today + TimeDelta::days(1), false),
            };
            tx.execute_batch(
                "DELETE FROM audience; DELETE FROM delivery; DELETE FROM files; DELETE FROM books; DELETE FROM schedule;",
            )?;
            tx.execute(
                "INSERT INTO schedule VALUES (?,?)",
                params![slot.timestamp(), started],
            )?;
            if started {
                for chat in chats { tx.execute("INSERT OR IGNORE INTO audience VALUES (?)",[chat])?; }
            }
            tx.commit()?;
            Ok(started.then_some(slot))
        })
        .await
    }

    pub(super) async fn audience(&self) -> Result<Vec<String>, String> {
        self.call(|c| {
            let mut st = c.prepare("SELECT chat FROM audience ORDER BY rowid")?;
            st.query_map([], |r| r.get(0))?.collect()
        })
        .await
    }

    /// Заявка перед сборкой. Черновые части после сбоя не отправляются и удаляются перед новой сборкой.
    pub(super) async fn begin(&self, report: &'static str, now: i64) -> Result<bool, String> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let n = tx.execute("INSERT INTO books VALUES (?1,1,?2,0)
                ON CONFLICT(report) DO UPDATE SET attempts=attempts+1,next_try=?3+min(3600,60 << min(attempts,6))
                WHERE ready=0 AND next_try<=?3",params![report,now+60,now])?;
            if n != 0 { tx.execute("DELETE FROM files WHERE report=?",[report])?; }
            tx.commit()?;
            Ok(n != 0)
        }).await
    }

    pub(super) async fn append(&self, report: &'static str, f: File) -> Result<(), String> {
        if f.bytes.len() > DOCUMENT_MAX_BYTES {
            return Err(format!("{} превышает лимит файла Telegram", f.name));
        }
        self.call(move |c| {
            c.execute(
                "INSERT INTO files(report,part,name,caption,data)
            SELECT ?1,coalesce(max(part),0)+1,?2,?3,?4 FROM files WHERE report=?1",
                params![report, f.name, f.caption, f.bytes],
            )
            .map(|_| ())
        })
        .await
    }

    pub(super) async fn seal(&self, report: &'static str) -> Result<(), String> {
        self.call(move |c| {
            c.execute("UPDATE books SET ready=1 WHERE report=?", [report])
                .map(|_| ())
        })
        .await
    }

    /// Только номера частей; байты читаются по одной при фактической отправке.
    pub(super) async fn pending(
        &self,
        chat: String,
        now: i64,
    ) -> Result<Vec<(String, u32)>, String> {
        self.call(move |c| {
            let mut stmt = c.prepare("SELECT f.report,f.part FROM files f JOIN books b USING(report)
                LEFT JOIN delivery d ON f.report=d.report AND f.part=d.part AND d.chat=?1
                WHERE b.ready=1 AND (d.status IS NULL OR (d.status IN ('retry','sending','rate_limited') AND d.retry <= ?2))
                ORDER BY b.rowid,f.part")?;
            stmt.query_map(params![chat,now],|r| Ok((r.get(0)?,r.get(1)?)))?.collect()
        }).await
    }

    /// COMMIT до HTTP. При неопределённом результате запроса Telegram не имеет idempotency key.
    pub(super) async fn claim(
        &self,
        chat: String,
        report: String,
        part: u32,
        now: i64,
    ) -> Result<Option<File>, String> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let n = tx.execute("INSERT INTO delivery VALUES (?1,?2,?3,1,'sending',?4+60)
                ON CONFLICT(chat,report,part) DO UPDATE SET attempts=attempts+1,status='sending',retry=?4+min(3600,60 << min(attempts,6))
                WHERE status IN ('retry','sending','rate_limited') AND retry<=?4", params![chat,report,part,now])?;
            if n == 0 { return Ok(None); }
            let f = tx.query_row("SELECT name,caption,data FROM files WHERE report=? AND part=?",
                params![report,part], |r| Ok(File {name:r.get(0)?,caption:r.get(1)?,bytes:r.get(2)?}))?;
            tx.commit()?;
            Ok(Some(f))
        }).await
    }

    pub(super) async fn finish(
        &self,
        chat: String,
        report: String,
        part: u32,
        status: &'static str,
        retry: i64,
    ) -> Result<(), String> {
        self.call(move |c| {
            let tx = c.transaction()?;
            tx.execute("UPDATE delivery SET status=?4,retry=max(retry,?5) WHERE chat=?1 AND report=?2 AND part=?3",
                params![chat,report,part,status,retry])?;
            if status == "sent" {
                tx.execute("DELETE FROM files WHERE report=?1 AND part=?2 AND EXISTS(SELECT 1 FROM audience)
                    AND NOT EXISTS(SELECT 1 FROM audience a WHERE NOT EXISTS(SELECT 1 FROM delivery d
                    WHERE d.chat=a.chat AND d.report=?1 AND d.part=?2 AND d.status='sent'))", params![report,part])?;
            }
            tx.commit()
        }).await
    }

    pub(super) async fn chat_waits(&self, chat: String, now: i64) -> Result<bool, String> {
        self.call(move |c| c.query_row("SELECT EXISTS(SELECT 1 FROM delivery WHERE chat=? AND status='rate_limited' AND retry>?)",
            params![chat,now], |r| r.get(0))).await
    }
}
