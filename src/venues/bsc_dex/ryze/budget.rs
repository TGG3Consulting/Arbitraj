//! Общий скользящий бюджет POST и GET Ryze Relayer для всех кошельков одной базы бота.
//! Router /quote использует другой сервис. Окно и Retry-After переживают перезапуск.
use crate::{storage::db::Db, venues::bsc_dex::ctx::Ctx};
use chrono::Utc;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

// Опубликованный потолок на IP за минуту: https://documentation.ryze.pro/relayer/overview
const IP_MAX: u64 = 100;
const WINDOW_MS: i64 = 60_000;
pub(super) fn limit(ctx: &Ctx) -> Option<u64> {
    ctx.int("ryze.relayer_requests_per_minute")
        .ok()
        .filter(|v| *v > 0)
        .map(|v| v.min(IP_MAX))
}

#[derive(Default, Deserialize, Serialize)]
struct Window {
    starts: Vec<i64>,
    blocked_until: i64,
}
enum Action {
    Request(u64),
    Pause(Option<u64>, Option<String>),
}
pub(super) const REJECTED: &str =
    "Ryze Relayer: HTTP 429, релей отклонил отправку по лимиту запросов";
impl Window {
    fn apply(&mut self, action: &Action, now: i64) -> Result<(), String> {
        self.starts.retain(|&t| t > now.saturating_sub(WINDOW_MS));
        match action {
            Action::Request(max) => {
                if self.blocked_until > now {
                    return Err(format!(
                        "Ryze Relayer: действует пауза HTTP 429, осталось {} мс",
                        self.blocked_until.saturating_sub(now)
                    ));
                }
                if self.starts.len() as u64 >= *max {
                    return Err(format!(
                        "Ryze Relayer: исчерпан общий бюджет {max} запросов за скользящую минуту"
                    ));
                }
                self.starts.push(now);
            }
            Action::Pause(secs, _) => {
                let ms = secs.map_or(WINDOW_MS, |s| {
                    i64::try_from(s.saturating_mul(1000)).unwrap_or(i64::MAX)
                });
                self.blocked_until = self.blocked_until.max(now.saturating_add(ms));
            }
        }
        Ok(())
    }
}

fn load(c: &rusqlite::Connection) -> rusqlite::Result<Window> {
    let raw: Option<String> = c
        .query_row(
            "SELECT body FROM core_state WHERE kind='ryze_relayer_window' AND id='ip'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s| serde_json::from_str(&s))
        .transpose()
        .map(|w| w.unwrap_or_default())
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}

/// Проверяет последнюю возможность отправки при выборе исполнителя. Бюджет расходует только фактический вызов.
pub(super) async fn available(ctx: &Ctx) -> Result<(), String> {
    let max = limit(ctx).ok_or("Ryze Relayer: не задан ryze.relayer_requests_per_minute")?;
    ctx.journal
        .db()
        .ledger()
        .read(move |c| Ok(load(c)?.apply(&Action::Request(max), Utc::now().timestamp_millis())))
        .await
        .map_err(|e| e.to_string())?
}
fn persist(
    connection: &mut rusqlite::Connection,
    action: Action,
) -> rusqlite::Result<Result<(), String>> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut window = load(&tx)?;
    let now = Utc::now();
    if let Err(reason) = window.apply(&action, now.timestamp_millis()) {
        return Ok(Err(reason));
    }
    let body = serde_json::to_string(&window)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    tx.execute(
        "INSERT INTO core_state(kind,id,body,updated_at) VALUES('ryze_relayer_window','ip',?1,?2)
         ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body,updated_at=excluded.updated_at",
        params![body, now.to_rfc3339()],
    )?;
    if let Action::Pause(retry_after, Some(claim)) = action {
        let proof = serde_json::json!({"status": 429, "retry_after_seconds": retry_after});
        tx.execute(
            "INSERT INTO core_state(kind,id,body,updated_at) VALUES('ryze_submit_429',?1,?2,?3)
             ON CONFLICT(kind,id) DO NOTHING",
            params![claim, proof.to_string(), now.to_rfc3339()],
        )?;
    }
    tx.commit()?;
    Ok(Ok(()))
}

async fn update(db: &Db, action: Action) -> Result<(), String> {
    db.ledger()
        .write(move |c| persist(c, action))
        .await
        .map_err(|e| format!("Ryze Relayer: ошибка учёта запросов: {e}"))?
}

/// Резервирует один реальный HTTP-запрос. Если окно занято, вызов немедленно получает причину отказа.
pub(super) async fn take(ctx: &Ctx) -> Result<(), String> {
    let max = limit(ctx).ok_or("Ryze Relayer: не задан ryze.relayer_requests_per_minute")?;
    update(ctx.journal.db(), Action::Request(max)).await
}
pub(super) async fn pause(
    db: &Db,
    seconds: Option<u64>,
    rejected: Option<String>,
) -> Result<(), String> {
    update(db, Action::Pause(seconds, rejected)).await
}

/// Только код ответа на исходный POST означает, что подписанное поручение было отклонено.
pub(super) async fn submit_rejected(db: &Db, claim: String) -> Result<bool, String> {
    db.ledger()
        .read(move |c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM core_state WHERE kind='ryze_submit_429' AND id=?1)",
                [claim],
                |r| r.get(0),
            )
        })
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_submit_consume_the_same_sliding_minute_and_backoff() {
        let mut w = Window::default();
        assert!(w.apply(&Action::Request(2), 10).is_ok());
        assert!(w.apply(&Action::Request(2), 20).is_ok());
        assert!(w.apply(&Action::Request(2), 60009).is_err());
        assert!(w.apply(&Action::Request(2), 60010).is_ok());
        assert!(w.apply(&Action::Pause(Some(90), None), 60010).is_ok());
        assert!(w.apply(&Action::Request(2), 150009).is_err());
        assert!(w.apply(&Action::Request(2), 150010).is_ok());
    }
    #[tokio::test]
    async fn concurrent_wallets_and_reopen_share_the_budget() {
        let dir =
            crate::venues::bsc_dex::hashflow::recovery_tests::directory("ryze-relayer-limit", 8453);
        let path = dir.join("state.sqlite");
        {
            let db = Db::open(&path).unwrap();
            let (a, b) = tokio::join!(
                update(&db, Action::Request(1)),
                update(&db, Action::Request(1))
            );
            assert_ne!(a.is_ok(), b.is_ok());
        }
        let db = Db::open(&path).unwrap();
        assert!(update(&db, Action::Request(1)).await.is_err());
    }
}
