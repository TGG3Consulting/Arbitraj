//! Файл: settings/live.rs
//! Что делает: изменения настроек на лету. Каждое изменение рассылается всем подписчикам;
//! компонент подписывается на нужные ему имена и применяет новое значение без перезапуска.
//! Кто вызывает: settings/store.rs (рассылает), компоненты и api/ws.rs (слушают).
//! От чего зависит: tokio::sync::broadcast, settings/value.rs.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

use super::value::Scope;

/// Одно изменение настройки.
#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub name: String,
    pub scope: Scope,
    /// Старое значение; None — не было задано.
    pub old: Option<Value>,
    /// Новое значение; None — очищено.
    pub new: Option<Value>,
    /// Кто поменял.
    pub by: String,
    pub at: DateTime<Utc>,
}

/// Подписка компонента на изменения своих настроек.
pub struct Watch {
    rx: broadcast::Receiver<Change>,
    /// Префиксы имён, которые интересны компоненту (например «notify.»).
    prefixes: Vec<String>,
}

impl Watch {
    /// Создаёт подписку.
    /// Принимает: приёмник изменений и префиксы имён. Пустой список — все изменения.
    pub fn new(rx: broadcast::Receiver<Change>, prefixes: Vec<String>) -> Self {
        Self { rx, prefixes }
    }

    /// Ждёт следующее подходящее изменение.
    /// Возвращает: Some(изменение) или None — рассылка закрыта (система останавливается).
    /// Если компонент отстал и пропустил изменения, пишет предупреждение и продолжает:
    /// актуальные значения он всё равно читает из хранилища при каждом использовании.
    pub async fn next(&mut self) -> Option<Change> {
        loop {
            match self.rx.recv().await {
                Ok(c) if self.matches(&c.name) => return Some(c),
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "подписчик настроек отстал");
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Подходит ли имя под префиксы подписки.
    fn matches(&self, name: &str) -> bool {
        self.prefixes.is_empty() || self.prefixes.iter().any(|p| name.starts_with(p.as_str()))
    }
}
