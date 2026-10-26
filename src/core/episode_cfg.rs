//! Файл: core/episode_cfg.rs
//! Что делает: настройки эпизодов (решение 74) и их хранение во времени: срок и лимит места. Не задана любая из
//! настроек episode.* — эпизоды не ведутся (причина видна на экране «Эпизоды» и одной записью в журнале на смену).
//! Удаление: закрытые эпизоды старше episode.keep_days; затем, пока файлы занимают больше episode.max_mb, — самые
//! старые закрытые. Открытый эпизод не удаляется; не помещается сам — его запись останавливается (episode_run.rs).
//! При запуске эпизоды, открытые до перезапуска, закрываются с причиной «перезапуск».
//! Кто вызывает: core/episode_run.rs (read, cleanup), core/engine.rs (load).
//! От чего зависит: episode.rs, storage/episodes_db.rs, storage/episode_files.rs, settings.
//! Настройки: episode.book_levels, episode.min_interval_ms, episode.max_mb, episode.keep_days, episode.zstd_level.

use chrono::{Duration, Utc};

use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::episodes_db;
use crate::storage::journal::Level;

use super::engine::{Core, lk};
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
}

impl Cfg {
    /// Что делает: читает настройки. Принимает: хранилище. Возвращает: Cfg. Ошибки: текст «не задана настройка …».
    pub fn read(s: &Store) -> Result<Cfg, String> {
        let int = |n: &str| s.integer(n, At::default()).map_err(|e| e.message);
        Ok(Cfg {
            book_levels: int("episode.book_levels")? as usize,
            min_interval_ms: int("episode.min_interval_ms")?,
            max_bytes: int("episode.max_mb")?.saturating_mul(1024 * 1024),
            keep_days: int("episode.keep_days")?,
            zstd_level: i32::try_from(int("episode.zstd_level")?)
                .map_err(|_| "episode.zstd_level вне пределов".to_string())?,
        })
    }
}

/// Что делает: настройки для работы; не заданы — запись в журнал один раз на смену причины, причина — для экрана.
/// Принимает: ядро. Возвращает: Some(Cfg) — эпизоды ведутся. Ошибок нет.
pub async fn current(core: &Core) -> Option<Cfg> {
    let r = Cfg::read(&core.inner.deps.settings);
    let why = r.as_ref().err().cloned();
    let changed = {
        let mut off = lk(&core.inner.episode_off);
        let c = *off != why;
        *off = why.clone();
        c
    };
    if changed && let Some(w) = &why {
        core.inner
            .deps
            .log(
                Level::Warn,
                "core.episode",
                None,
                format!("эпизоды не ведутся: {w}"),
                None,
            )
            .await;
    }
    r.ok()
}

/// Что делает: удаляет закрытые эпизоды по сроку и месту (см. шапку). Принимает: ядро, настройки. Ошибок нет —
/// сбой базы или диска пишется в журнал.
pub async fn cleanup(core: &Core, cfg: &Cfg) {
    let (db, files) = (&core.inner.deps.db, &core.inner.episode_files);
    let rows = match episodes_db::oldest_first(db).await {
        Ok(r) => r,
        Err(e) => return warn(core, format!("список эпизодов: {e}")).await,
    };
    let old = Utc::now() - Duration::days(cfg.keep_days as i64);
    for body in rows {
        let Ok(e) = serde_json::from_str::<EpisodeInfo>(&body) else {
            continue;
        };
        let Some(end) = e.ended_at else { continue };
        if end >= old && files.used() <= cfg.max_bytes {
            continue;
        }
        if let Err(err) = files.remove(&e.id) {
            warn(core, format!("эпизод {}: файл не удалён: {err}", e.id)).await;
            continue;
        }
        if let Err(err) = episodes_db::delete(db, &e.id).await {
            warn(core, format!("эпизод {}: {err}", e.id)).await;
        }
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
async fn warn(core: &Core, text: String) {
    core.inner
        .deps
        .log(Level::Warn, "core.episode", None, text, None)
        .await;
}
