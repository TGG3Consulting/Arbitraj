//! Файл: core/deps.rs
//! Что делает: всё, чем пользуется ядро снаружи (шлюз, настройки, журнал, уведомления, база,
//! поток изменений настроек), и короткие помощники: запись в журнал и уведомление одной строкой.
//! Кто вызывает: core/engine.rs (создаёт), все модули ядра (пользуются).
//! От чего зависит: gateway/registry, settings, storage (db, journal), notify/dispatch.
//! Настройки: нет.
//!
//! Ядро не знает о конкретных площадках: к ним — только через реестр шлюза (gateway/registry.rs).

use serde_json::Value;
use tokio::sync::broadcast;

use crate::gateway::registry::Registry;
use crate::notify::dispatch::Notifier;
use crate::settings::live::{Change, Watch};
use crate::settings::store::Store;
use crate::storage::db::Db;
use crate::storage::journal::{Journal, Level};

/// Зависимости ядра. Дешёво клонируются.
#[derive(Clone)]
pub struct Deps {
    /// Реестр адаптеров шлюза — единственный путь к площадкам.
    pub gw: Registry,
    pub settings: Store,
    pub journal: Journal,
    pub notifier: Notifier,
    pub db: Db,
    /// Поток изменений настроек — чтобы ждать, пока Тигран задаст нужную настройку.
    pub changes: broadcast::Sender<Change>,
    /// Размер очередей ядра (аргумент запуска --live-buffer, cli.rs).
    pub buffer: usize,
    /// Папка данных (аргумент запуска --data-dir): файлы эпизодов — <data_dir>/episodes (решение 74).
    pub data_dir: std::path::PathBuf,
}

impl Deps {
    /// Пишет запись в журнал (вид — «core.<что>»). Ошибка журнала уже выведена в лог процесса.
    /// Принимает: уровень, вид, площадку (если есть), текст, данные.
    pub async fn log(
        &self,
        level: Level,
        kind: &str,
        venue: Option<&str>,
        message: impl Into<String>,
        data: Option<Value>,
    ) {
        let message = message.into();
        match level {
            Level::Error => tracing::error!(kind, venue, "{message}"),
            Level::Warn => tracing::warn!(kind, venue, "{message}"),
            Level::Info => tracing::info!(kind, venue, "{message}"),
        }
        let _ = self.journal.write(level, kind, venue, message, data).await;
    }

    /// Уведомление в Telegram по коду из notify/catalog.rs (уйдёт, только если событие включено).
    /// Принимает: код события, площадку, текст. Ошибок нет: всё пишет notify в журнал.
    pub async fn notify(&self, code: &str, venue: Option<&str>, text: &str) {
        self.notifier.send(code, venue, text).await;
    }

    /// Подписка на изменения настроек с нужными префиксами имён. Пустой список — все изменения.
    pub fn watch(&self, prefixes: &[&str]) -> Watch {
        Watch::new(
            self.changes.subscribe(),
            prefixes.iter().map(|p| p.to_string()).collect(),
        )
    }
}
