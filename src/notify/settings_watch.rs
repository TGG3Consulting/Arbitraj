//! Файл: notify/settings_watch.rs
//! Что делает: слушает изменения настроек на лету, пишет каждое в журнал
//! и шлёт событие «system.settings_changed» (кто, что и когда поменял).
//! Кто вызывает: main.rs (запускает задачу).
//! От чего зависит: settings/live.rs, notify/dispatch.rs, storage/journal.
//! Настройки: событие system.settings_changed включается в интерфейсе.

use serde_json::json;

use crate::settings::live::Watch;
use crate::storage::journal::Level;

use super::dispatch::Notifier;

/// Работает, пока система запущена.
/// Принимает: подписку на все изменения и отправителя уведомлений. Ошибок не возвращает.
pub async fn run(mut watch: Watch, notifier: Notifier) {
    while let Some(c) = watch.next().await {
        let shown = |v: &Option<serde_json::Value>| {
            v.as_ref()
                .map(|x| x.to_string())
                .unwrap_or_else(|| "не задано".into())
        };
        let text = format!(
            "{} [{}]: {} → {} (кто: {})",
            c.name,
            c.scope.to_key(),
            shown(&c.old),
            shown(&c.new),
            c.by
        );
        let data =
            json!({ "name": c.name, "scope": c.scope, "old": c.old, "new": c.new, "by": c.by });
        let _ = notifier
            .journal
            .write(
                Level::Info,
                "settings.changed",
                None,
                text.clone(),
                Some(data),
            )
            .await;
        notifier.send("system.settings_changed", None, &text).await;
    }
}
