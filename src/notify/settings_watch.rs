//! Файл: notify/settings_watch.rs
//! Что делает: слушает изменения настроек на лету и шлёт событие «system.settings_changed»
//! (кто, что и когда поменял). Журнал изменений пишет само хранилище (settings/store.rs),
//! поэтому медленный Telegram не может привести к потере записей журнала.
//! Кто вызывает: main.rs (запускает задачу).
//! От чего зависит: settings/live.rs, notify/dispatch.rs.
//! Настройки: событие system.settings_changed включается в интерфейсе.

use crate::settings::live::Watch;

use super::dispatch::Notifier;

/// Работает, пока система запущена. Каждая отправка — в своей задаче, чтобы не задерживать приём.
/// Принимает: подписку на все изменения и отправителя уведомлений. Ошибок не возвращает.
pub async fn run(mut watch: Watch, notifier: Notifier) {
    while let Some(c) = watch.next().await {
        let n = notifier.clone();
        tokio::spawn(async move { n.send("system.settings_changed", None, &c.describe()).await });
    }
}
