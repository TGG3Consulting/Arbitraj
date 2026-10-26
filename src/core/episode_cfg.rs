//! Файл: core/episode_cfg.rs
//! Что делает: настройки эпизодов (решение 74) и их хранение во времени. Не задана любая из обязательных episode.* —
//! эпизоды не ведутся (причина видна на экране «Эпизоды» и одной записью в журнале на смену); episode.min_notify_ms
//! необязательна. Уборка (cleanup) — из задачи эпизодов при запуске и раз в episode.cleanup_s: закрытые эпизоды старше
//! episode.keep_days; затем, пока файлы занимают больше episode.max_mb, — самые старые закрытые; файлы без строки в базе
//! (сироты) — всегда. Открытый эпизод не удаляется. Файловые операции — в spawn_blocking. При запуске эпизоды, открытые
//! до перезапуска, закрываются с причиной «перезапуск» (load).
//! Кто вызывает: core/episode_run.rs (Cfg::read), core/episode_worker.rs (cleanup, load).
//! От чего зависит: episode.rs, storage/episodes_db.rs, storage/episode_files.rs, settings.
//! Настройки: episode.book_levels, episode.min_interval_ms, episode.max_mb, episode.keep_days, episode.zstd_level,
//! episode.cleanup_s, episode.min_notify_ms (необязательна).

use std::collections::HashSet;

use chrono::{Duration, Utc};

use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::episodes_db;
use crate::storage::journal::Level;

use super::engine::Core;
use super::episode::EpisodeInfo;

/// Настройки эпизодов.
#[derive(Debug, Clone)]
pub struct Cfg {
    /// Сколько верхних уровней стакана писать в запись.
    pub book_levels: usize,
    /// Записи открытого эпизода — не чаще, мс.
    pub min_interval_ms: u64,
    /// Лимит места всех файлов эпизодов, байт.
    pub max_bytes: u64,
    /// Срок хранения закрытых эпизодов, дней.
    pub keep_days: u64,
    /// Уровень сжатия zstd.
    pub zstd_level: i32,
    /// Как часто уборка (срок, место, сироты, пропавшие направления), секунды.
    pub cleanup_s: u64,
    /// Эпизод короче — в Telegram ни открытия, ни закрытия, ни deal.candidate; None — уведомления сразу.
    pub min_notify_ms: Option<u64>,
}

impl Cfg {
    /// Что делает: читает настройки. Принимает: хранилище. Возвращает: Cfg. Ошибки: текст «не задана настройка …».
    pub fn read(s: &Store) -> Result<Cfg, String> {
        let int = |n: &str| s.integer(n, At::default()).map_err(|e| e.message);
        let min_notify_ms = match s.integer("episode.min_notify_ms", At::default()) {
            Ok(v) => Some(v),
            Err(e) if e.code == ErrorCode::NotConfigured => None,
            Err(e) => return Err(e.message),
        };
        Ok(Cfg {
            book_levels: int("episode.book_levels")? as usize,
            min_interval_ms: int("episode.min_interval_ms")?,
            max_bytes: int("episode.max_mb")?.saturating_mul(1024 * 1024),
            keep_days: int("episode.keep_days")?,
            zstd_level: i32::try_from(int("episode.zstd_level")?)
                .map_err(|_| "episode.zstd_level вне пределов".to_string())?,
            cleanup_s: int("episode.cleanup_s")?,
            min_notify_ms,
        })
    }
}

/// Что делает: уборка по сроку, месту и сиротам (см. шапку). Принимает: ядро, настройки, id открытых эпизодов.
/// Ошибок нет — сбой базы или диска пишется в журнал.
pub async fn cleanup(core: &Core, cfg: &Cfg, open: &HashSet<String>) {
    let (db, files) = (&core.inner.deps.db, core.inner.episode_files.clone());
    let rows = match episodes_db::oldest_first(db).await {
        Ok(r) => r,
        Err(e) => return warn(core, format!("список эпизодов: {e}")).await,
    };
    let list: Vec<EpisodeInfo> = rows
        .iter()
        .filter_map(|b| serde_json::from_str(b).ok())
        .collect();
    // Сироты: файл есть, строки в базе нет, эпизод не открыт.
    let known: HashSet<String> = list
        .iter()
        .map(|e| e.id.clone())
        .chain(open.iter().cloned())
        .collect();
    let f = files.clone();
    let orphans = tokio::task::spawn_blocking(move || f.orphans(&known))
        .await
        .unwrap_or_default();
    for id in orphans {
        remove(core, &id, false).await;
    }
    let old = Utc::now() - Duration::days(cfg.keep_days as i64);
    for e in list {
        let Some(end) = e.ended_at else { continue };
        if open.contains(&e.id) || (end >= old && files.used() <= cfg.max_bytes) {
            continue;
        }
        remove(core, &e.id, true).await;
    }
}

/// Удаляет файл (в spawn_blocking) и, если надо, строку базы. Ошибок нет — в журнал.
async fn remove(core: &Core, id: &str, row: bool) {
    let f = core.inner.episode_files.clone();
    let i = id.to_string();
    match tokio::task::spawn_blocking(move || f.remove(&i)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return warn(core, format!("эпизод {id}: файл не удалён: {e}")).await,
        Err(e) => return warn(core, format!("эпизод {id}: {e}")).await,
    }
    if row && let Err(e) = episodes_db::delete(&core.inner.deps.db, id).await {
        warn(core, format!("эпизод {id}: {e}")).await;
    }
}

/// Что делает: при запуске закрывает эпизоды, оставшиеся открытыми до перезапуска. Принимает: ядро. Ошибок нет.
pub async fn load(core: &Core) {
    let db = &core.inner.deps.db;
    let Ok(rows) = episodes_db::oldest_first(db).await else {
        return;
    };
    for body in rows {
        let Ok(mut e) = serde_json::from_str::<EpisodeInfo>(&body) else {
            continue;
        };
        if e.ended_at.is_some() {
            continue;
        }
        e.ended_at = Some(Utc::now());
        e.close_reason = Some("перезапуск: эпизод прерван, запись до остановки — в файле".into());
        let started = e.started_at.to_rfc3339();
        if let Ok(b) = serde_json::to_string(&e) {
            let _ = episodes_db::save(db, &e.id, started, b).await;
        }
    }
}

/// Запись в журнал о сбое хранения эпизодов. Ошибок нет.
pub async fn warn(core: &Core, text: String) {
    core.inner
        .deps
        .log(Level::Warn, "core.episode", None, text, None)
        .await;
}
