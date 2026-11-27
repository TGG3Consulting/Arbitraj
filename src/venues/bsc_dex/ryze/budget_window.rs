//! Атомарное скользящее окно для кошельков одной базы. У релея сохраняется исторический ключ IP, у Router окно на origin.
use crate::storage::db::Db;
use chrono::Utc;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::{LazyLock, RwLock}};

const WINDOW_MS: i64 = 60_000;

pub(super) enum Key {
    Relayer,
    Router(String),
}

impl Key {
    fn kind(&self) -> &'static str {
        match self {
            Self::Relayer => "ryze_relayer_window",
            Self::Router(_) => "ryze_router_window",
        }
    }
    fn id(&self) -> &str {
        match self {
            Self::Relayer => "ip",
            Self::Router(origin) => origin,
        }
    }
    fn service(&self) -> &'static str {
        match self {
            Self::Relayer => "Ryze Relayer",
            Self::Router(_) => "Ryze Router",
        }
    }
}
#[derive(Default, Deserialize, Serialize)]
struct Window {
    starts: Vec<i64>,
    blocked_until: i64,
}
// Последнее прочитанное окно конкретного сервера. Пустая книга при старте не требует загрузки всех окон.
static ROUTERS: LazyLock<RwLock<HashMap<String, Window>>> = LazyLock::new(Default::default);

/// Быстрый предварительный отказ до обращения в SQLite или к каноническому узлу. Окончательное разрешение даёт persist.
pub(super) fn router_ready(origin: &str, max: u64) -> bool {
    let views = ROUTERS.read().unwrap_or_else(|e| e.into_inner());
    let Some(window) = views.get(origin) else { return true };
    let now = Utc::now().timestamp_millis();
    let expired = window.starts.partition_point(|t| *t <= now.saturating_sub(WINDOW_MS));
    window.blocked_until <= now && ((window.starts.len() - expired) as u64) < max
}

pub(super) fn router_unpaused(origin: &str) -> bool {
    ROUTERS.read().unwrap_or_else(|e| e.into_inner()).get(origin)
        .is_none_or(|w| w.blocked_until <= Utc::now().timestamp_millis())
}

fn deadline(now: i64, seconds: Option<u64>) -> i64 {
    let ms = seconds.map_or(WINDOW_MS, |s| {
        i64::try_from(s.saturating_mul(1000)).unwrap_or(i64::MAX)
    });
    now.saturating_add(ms)
}

/// Ответ 429 закрывает origin сразу, даже если запись в SQLite не сможет завершиться.
pub(super) fn block_router(origin: &str, seconds: Option<u64>) {
    let mut windows = ROUTERS.write().unwrap_or_else(|e| e.into_inner());
    let current = windows.entry(origin.to_owned()).or_default();
    current.blocked_until = current.blocked_until.max(deadline(Utc::now().timestamp_millis(), seconds));
}

#[cfg(test)]
pub(super) fn forget_router(origin: &str) {
    ROUTERS.write().unwrap_or_else(|e| e.into_inner()).remove(origin);
}

fn remember(key: &Key, mut window: Window) {
    if let Key::Router(origin) = key {
        // Обычно starts уже отсортирован. Учитываем и корректировку часов назад.
        if !window.starts.is_sorted() {
            window.starts.sort_unstable();
        }
        let mut views = ROUTERS.write().unwrap_or_else(|e| e.into_inner());
        if !views.contains_key(origin) {
            let now = Utc::now().timestamp_millis();
            views.retain(|_, w| w.blocked_until > now
                || w.starts.last().is_some_and(|t| *t > now.saturating_sub(WINDOW_MS)));
        }
        let current = views.entry(origin.clone()).or_default();
        window.blocked_until = window.blocked_until.max(current.blocked_until);
        *current = window;
    }
}

pub(super) enum Action {
    Request(u64),
    Pause(Option<u64>, Option<String>),
}
impl Window {
    fn apply(&mut self, action: &Action, now: i64, service: &str) -> Result<(), String> {
        self.starts.retain(|&t| t > now.saturating_sub(WINDOW_MS));
        match action {
            Action::Request(max) => {
                if self.blocked_until > now {
                    return Err(format!(
                        "{service}: действует пауза HTTP 429, осталось {} мс",
                        self.blocked_until.saturating_sub(now)
                    ));
                }
                if self.starts.len() as u64 >= *max {
                    return Err(format!(
                        "{service}: исчерпан общий бюджет {max} запросов за скользящую минуту"
                    ));
                }
                self.starts.push(now);
            }
            Action::Pause(secs, _) => {
                self.blocked_until = self.blocked_until.max(deadline(now, *secs));
            }
        }
        Ok(())
    }
}

fn load(c: &rusqlite::Connection, key: &Key) -> rusqlite::Result<Window> {
    let raw: Option<String> = c
        .query_row(
            "SELECT body FROM core_state WHERE kind=?1 AND id=?2",
            params![key.kind(), key.id()],
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

fn persist(
    connection: &mut rusqlite::Connection,
    action: Action,
    key: &Key,
) -> rusqlite::Result<Result<(), String>> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let (Key::Router(origin), Action::Request(max)) = (key, &action)
        && !router_ready(origin, *max)
    {
        return Ok(Err("Ryze Router: действует общее ограничение запросов".into()));
    }
    let mut window = load(&tx, key)?;
    let now = Utc::now();
    if let Err(reason) = window.apply(&action, now.timestamp_millis(), key.service()) {
        remember(key, window);
        return Ok(Err(reason));
    }
    let body = serde_json::to_string(&window)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    tx.execute(
        "INSERT INTO core_state(kind,id,body,updated_at) VALUES(?1,?2,?3,?4)
         ON CONFLICT(kind,id) DO UPDATE SET body=excluded.body,updated_at=excluded.updated_at",
        params![key.kind(), key.id(), body, now.to_rfc3339()],
    )?;
    if let Action::Pause(retry_after, Some(claim)) = action {
        let proof = serde_json::json!({"status": 429, "retry_after_seconds": retry_after});
        tx.execute(
            "INSERT INTO core_state(kind,id,body,updated_at) VALUES('ryze_submit_429',?1,?2,?3)
             ON CONFLICT(kind,id) DO NOTHING",
            params![claim, proof.to_string(), now.to_rfc3339()],
        )?;
    }
    // Публикация внутри IMMEDIATE-транзакции сохраняет порядок между двумя соединениями.
    // При сбое commit предварительный отказ консервативен, успешный HTTP всё равно требует commit.
    remember(key, window);
    tx.commit()?;
    Ok(Ok(()))
}

pub(super) async fn update(db: &Db, key: Key, action: Action) -> Result<(), String> {
    let service = key.service();
    db.ledger()
        .write(move |c| persist(c, action, &key))
        .await
        .map_err(|e| format!("{service}: ошибка учёта запросов: {e}"))?
}

pub(super) async fn available(db: &Db, key: Key, max: u64) -> Result<(), String> {
    db.ledger().read(move |c| {
        Ok(load(c, &key)?.apply(&Action::Request(max), Utc::now().timestamp_millis(), key.service()))
    }).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "window_tests.rs"]
mod tests;
