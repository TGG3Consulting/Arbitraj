//! Файл: notify/press_run.rs
//! Что делает: «Да» из Telegram → команда ядру тем же путём, что у экрана (Core::command, источник — Telegram, кто
//! нажал); отметка записи «выполнена из Telegram» до команды (второе нажатие — «уже выполнено»), снятие при отказе;
//! итог — «Выполнено: …» / «Отклонено: …», решение 408 — «Ждёт: …» (команда ушла ждать окончания сделок монеты,
//! ID ожидания — второй элемент итога). Вынесено из press_confirm.rs — размер файла.
//! Кто вызывает: notify/press_confirm.rs (confirm), notify/ctl/progress.rs (перевод USDT).
//! От чего зависит: core (Core::command, manual_wait::deferred), mirror_state.rs, storage/journal.
//! Настройки: нет.

use chrono::Utc;
use serde_json::json;

use crate::core::Core;
use crate::core::types::{CommandSource, ManualCommand};
use crate::storage::journal::{Journal, Level};

use super::mirror_state::{Pending, Shared, lock};
use super::tg_buttons::User;

/// Команда ожидания ядру тем же путём, что у экрана (Core::command, источник — Telegram, кто нажал), отметка записи
/// выполненной, итог в журнал. Принимает: ядро, журнал, память дубля, ожидание, кто, чат.
/// Возвращает: строку итога «Выполнено: …» или «Отклонено: …». Ошибок нет.
pub async fn run_pending(
    core: &Core,
    journal: &Journal,
    state: &Shared,
    p: &Pending,
    from: &User,
    chat: i64,
) -> String {
    run_pending_full(core, journal, state, p, from, chat)
        .await
        .0
}

/// Как run_pending; решение 408: ещё и ID ожидания, если команда ушла ждать окончания сделок монеты (решение 405) —
/// итог тогда «Ждёт: …», а не «Выполнено: …»; отметка записи «выполнена из Telegram» держится до конца ожидания
/// (notify/mirror_wait.rs снимает её при конце без выполнения). Возвращает: (итог, ID ожидания). Ошибок нет.
pub async fn run_pending_full(
    core: &Core,
    journal: &Journal,
    state: &Shared,
    p: &Pending,
    from: &User,
    chat: i64,
) -> (String, Option<String>) {
    let who = from.describe();
    let cmd = ManualCommand {
        action: p.action.clone(),
        source: CommandSource::Telegram {
            chat: chat.to_string(),
        },
        by: Some(who.clone()),
        at: Utc::now(),
    };
    // Запись отмечается выполненной ДО команды: второе нажатие во время выполнения — «уже выполнено».
    // Ушла с экрана — отметку снимает mirror_state (запись с тем же ключом может вернуться); отказ — снимаем здесь.
    if let Some(k) = &p.key {
        lock(state).done.insert(k.clone());
    }
    let res = core.command(cmd).await;
    if res.is_err()
        && let Some(k) = &p.key
    {
        lock(state).done.remove(k);
    }
    let at = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let wait = match &res {
        Ok(_) => crate::core::manual_wait::deferred(core, &p.action),
        Err(_) => None,
    };
    let (level, text) = match res {
        Ok(t) if wait.is_some() => (
            Level::Info,
            format!("Ждёт: {who}, «{}» — {t}, {at}.", p.title),
        ),
        Ok(t) => (
            Level::Info,
            format!("Выполнено: {who}, «{}» — {t}, {at}.", p.title),
        ),
        Err(e) => (
            Level::Warn,
            format!("Отклонено: {who}, «{}» — {e}, {at}.", p.title),
        ),
    };
    let data = json!({ "action": p.action, "from": who, "chat": chat });
    let _ = journal
        .write(
            level,
            "notify",
            None,
            format!("Telegram: {text}"),
            Some(data),
        )
        .await;
    (text, wait)
}
