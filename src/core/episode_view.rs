//! Файл: core/episode_view.rs
//! Что делает: данные экрана «Эпизоды» (решение 74): список эпизодов (сводки из SQLite, открытые — с живыми счётчиками),
//! почему эпизоды не ведутся (не заданы episode.*), занятое место; записи одного эпизода из его файла; путь файла
//! для кнопки «Скачать».
//! Кто вызывает: api/episodes.rs.
//! От чего зависит: episode.rs, episode_cfg.rs, storage/episodes_db.rs, storage/episode_files.rs.
//! Настройки: episode.* (только показ причины, если не заданы).

use serde_json::{Value, json};

use crate::storage::{episode_files, episodes_db};

use super::engine::{Core, lk};
use super::episode::EpisodeInfo;
use super::episode_cfg::Cfg;

impl Core {
    /// Что делает: список эпизодов, новые сверху. Принимает: сколько не больше.
    /// Возвращает: {episodes: [EpisodeInfo], off: причина | null, used_bytes}. Ошибки: текст сбоя базы.
    pub async fn episodes(&self, limit: u32) -> Result<Value, String> {
        let rows = episodes_db::list(&self.inner.deps.db, limit)
            .await
            .map_err(|e| e.to_string())?;
        let open = lk(&self.inner.episodes).list();
        let list: Vec<EpisodeInfo> = rows
            .iter()
            .filter_map(|b| serde_json::from_str::<EpisodeInfo>(b).ok())
            .map(|e| open.iter().find(|o| o.id == e.id).cloned().unwrap_or(e))
            .collect();
        let off = Cfg::read(&self.inner.deps.settings).err();
        Ok(json!({
            "episodes": list,
            "off": off,
            "used_bytes": self.inner.episode_files.used(),
        }))
    }

    /// Что делает: сводка и записи эпизода. Принимает: id, сколько записей не больше.
    /// Возвращает: {info, records: [JSON], truncated}; None — нет такого эпизода. Ошибки: текст (база, файл).
    pub async fn episode(&self, id: &str, max: usize) -> Result<Option<Value>, String> {
        let Some(body) = episodes_db::get(&self.inner.deps.db, id)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        let info: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let path = self.inner.episode_files.path(id);
        // Чтение и распаковка — блокирующие, вне потоков async.
        let p = path.clone();
        let (lines, truncated) = tokio::task::spawn_blocking(move || episode_files::read(&p, max))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("файл эпизода {}: {e}", path.display()))?;
        let records: Vec<Value> = lines
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        Ok(Some(
            json!({ "info": info, "records": records, "truncated": truncated }),
        ))
    }

    /// Путь файла эпизода (для скачивания). Принимает: id (только буквы, цифры, «-»). Возвращает: путь; None —
    /// неверный id. Ошибок нет.
    pub fn episode_path(&self, id: &str) -> Option<std::path::PathBuf> {
        let ok = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        ok.then(|| self.inner.episode_files.path(id))
    }
}
