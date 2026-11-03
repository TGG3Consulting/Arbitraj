//! Файл: core/window_cfg.rs
//! Что делает: настройки окон (решение 125) и их хранение во времени. window.enabled = нет или не задана любая из
//! window.* — окна не пишутся (причина — на экране «Окна» и одной записью в журнале на смену). Уборка (cleanup) — из
//! задачи окон при запуске и раз в window.cleanup_s: закрытые окна старше window.keep_days; затем, пока файлы занимают
//! больше window.max_mb, — самые старые закрытые; файлы без строки в базе (сироты) — всегда. Открытое окно не
//! удаляется. При запуске окна, открытые до перезапуска, закрываются с причиной «перезапуск» (load).
//! Кто вызывает: core/window_run.rs (Cfg::read), core/window_worker.rs (cleanup, load), core/window_view.rs.
//! От чего зависит: window.rs, storage/windows_db.rs, storage/episode_files.rs (файлы окон), settings.
//! Настройки: window.enabled, window.min_hold_ms, window.queue_max, window.max_mb, window.keep_days,
//! window.zstd_level, window.cleanup_s.

use std::collections::HashSet;

use chrono::{Duration, Utc};

use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::Level;
use crate::storage::windows_db;

use super::engine::Core;
use super::window::WindowInfo;

/// Настройки окон.
#[derive(Debug, Clone)]
pub struct Cfg {
    pub min_hold_ms: u64,
    pub queue_max: usize,
    pub max_bytes: u64,
    pub keep_days: u64,
    pub zstd_level: i32,
    pub cleanup_s: u64,
}

impl Cfg {
    /// Что делает: читает настройки. Принимает: хранилище. Возвращает: Cfg.
    /// Ошибки: текст «не задана настройка …» или «window.enabled = нет».
    pub fn read(s: &Store) -> Result<Cfg, String> {
        let int = |n: &str| s.integer(n, At::default()).map_err(|e| e.message);
        if !s
            .flag("window.enabled", At::default())
            .map_err(|e| e.message)?
        {
            return Err("window.enabled = нет".into());
        }
        Ok(Cfg {
            min_hold_ms: int("window.min_hold_ms")?,
            queue_max: int("window.queue_max")? as usize,
            max_bytes: int("window.max_mb")?.saturating_mul(1024 * 1024),
            keep_days: int("window.keep_days")?,
            zstd_level: i32::try_from(int("window.zstd_level")?)
                .map_err(|_| "window.zstd_level вне пределов".to_string())?,
            cleanup_s: int("window.cleanup_s")?,
        })
    }
}

/// Сводки базы, старые сверху. Ошибок нет — сбой базы в журнал (пусто).
async fn rows(core: &Core) -> Vec<WindowInfo> {
    match windows_db::oldest_first(&core.inner.deps.db).await {
        Ok(r) => r
            .iter()
            .filter_map(|b| serde_json::from_str(b).ok())
            .collect(),
        Err(e) => {
            warn(core, format!("список окон: {e}")).await;
            vec![]
        }
    }
}

/// Что делает: уборка по сроку, месту и сиротам (см. шапку). Принимает: ядро, настройки, id открытых окон.
/// Ошибок нет — сбой базы или диска пишется в журнал.
pub async fn cleanup(core: &Core, cfg: &Cfg, open: &HashSet<String>) {
    let files = core.inner.window_files.clone();
    let list = rows(core).await;
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
    let f = core.inner.window_files.clone();
    let i = id.to_string();
    match tokio::task::spawn_blocking(move || f.remove(&i)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return warn(core, format!("окно {id}: файл не удалён: {e}")).await,
        Err(e) => return warn(core, format!("окно {id}: {e}")).await,
    }
    if row && let Err(e) = windows_db::delete(&core.inner.deps.db, id).await {
        warn(core, format!("окно {id}: {e}")).await;
    }
}

/// Что делает: при запуске закрывает окна, оставшиеся открытыми до перезапуска. Принимает: ядро. Ошибок нет.
pub async fn load(core: &Core) {
    for mut e in rows(core).await {
        if e.ended_at.is_some() {
            continue;
        }
        e.ended_at = Some(Utc::now());
        e.close_reason = Some("перезапуск: окно прервано, запись до остановки — в файле".into());
        e.bytes = core.inner.window_files.size(&e.id);
        super::window_close::save_row(core, &e).await;
    }
}

/// Запись в журнал о сбое хранения окон. Ошибок нет.
pub async fn warn(core: &Core, text: String) {
    core.inner
        .deps
        .log(Level::Warn, "core.window", None, text, None)
        .await;
}
