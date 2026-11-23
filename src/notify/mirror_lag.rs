//! Файл: notify/mirror_lag.rs
//! Что делает: решение 334 (задача t155) — дубль «Ручного управления» (mirror.rs) отстал от рассылки событий ядра
//! (broadcast CoreEvent, ёмкость --live-buffer; RecvError::Lagged — пропущено n событий). Среди них мог быть итог
//! команды (CoreEvent::Command) — сообщение «Выполнено: кто, что, итог» в Telegram не придёт; сами записи экрана
//! сверяются на каждом круге (reconcile) и не теряются. Раньше отставание молча пропускалось — теперь счёт и запись
//! журнала notify.core_events_lost (gateway/lost_log.rs: на 1-ю, 2-ю, 4-ю… или по интервалу).
//! Кто вызывает: notify/mirror.rs (run).
//! От чего зависит: notify/dispatch.rs (настройки и журнал Notifier), gateway/lost_log.rs.
//! Настройки: log.events_lost_interval_s (через lost_log.rs).

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::gateway::lost_log::{Lost, Note};

use super::dispatch::Notifier;

/// Вид записи журнала.
pub const LOST_KIND: &str = "notify.core_events_lost";

/// Счёт отставаний (один на процесс: дубль один).
static LOST: LazyLock<Lost> = LazyLock::new(Lost::default);

/// Отставание дубля от событий ядра: счёт и запись журнала. Принимает: уведомитель (настройки, журнал), сколько
/// событий пропущено. Ошибок нет.
pub fn lagged(n: &Notifier, skipped: u64) {
    static SKIPPED: AtomicU64 = AtomicU64::new(0);
    let total = SKIPPED.fetch_add(skipped, Ordering::Relaxed) + skipped;
    let note = Note {
        kind: LOST_KIND,
        venue: None,
        what: "дубль «Ручного управления» отстал от итогов команд — «Выполнено» могло не прийти в Telegram",
        last: format!("пропущено итогов команд за это отставание: {skipped}, с запуска: {total}"),
    };
    LOST.hit(&n.settings, &n.journal, note);
}
