//! Файл: gateway/book_lost_log.rs
//! Что делает: решение 324 — запись журнала уровня Warn `venue.book_events_lost` о стаканах биржи, не вошедших в канал
//! событий ядра сразу (канал полон): «отложено, заменено более свежими, доставлено, потеряно, ждут» с прошлой записи —
//! не чаще раза в log.pool_events_lost_interval_s секунд (первая — сразу; та же настройка, что у пулов DEX — решение
//! 313, новых настроек журнала нет). Не задана — записей нет (счёт в наборе копится и уйдёт первой записью).
//! Пишется при каждом не сразу отправленном кадре (если пора) и когда общий набор опустел (крючок core_out.rs).
//! Кто вызывает: gateway/core_out_book.rs.
//! От чего зависит: gateway/core_out.rs (счёт класса, часы), settings/store.rs, storage/journal.rs, tokio (запись
//! отдельной задачей).
//! Настройки: log.pool_events_lost_interval_s (вся система или площадка; необязательная).

use std::time::Duration;

use serde_json::json;

use crate::core::types::VenueId;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};

use super::core_out::Feed;
use super::core_out_item::Class;
use super::keep_last::Counts;

/// Имя настройки интервала записи (общая с пулами DEX, решение 313).
const INTERVAL: &str = "log.pool_events_lost_interval_s";

/// Если пора — запись счёта класса «стаканы площадки» (счёт обнуляется). Принимает: набор, площадку, настройки,
/// журнал. Ошибок нет; нет рантайма tokio — записи нет.
pub fn note(feed: &Feed, venue: &VenueId, settings: &Store, journal: &Journal) {
    let class = Class::Book(venue.clone());
    let read = || {
        settings
            .resolve(INTERVAL, Some(&venue.0), None)
            .and_then(|v| v.as_u64())
            .map(Duration::from_secs)
    };
    // Нечего сказать (всё уходило сразу) — часы не трогаются, записи нет.
    if feed.peek(&class) == Counts::default() {
        return;
    }
    let Some(since) = feed.due(&class, read) else {
        return;
    };
    let c = feed.take(&class);
    let window = since.map_or("с запуска".to_string(), |d| {
        format!("за {} с", d.as_secs())
    });
    let msg = format!(
        "стаканы: канал событий ядра полон — отложено {} (venue.book_events_keep), заменено более свежими {}, \
         доставлено {}, потеряно {}, ждут {} {window}",
        c.deferred, c.replaced, c.delivered, c.lost, c.waiting
    );
    let data = json!({
        "deferred": c.deferred, "replaced": c.replaced, "delivered": c.delivered, "lost": c.lost,
        "waiting": c.waiting, "since_s": since.map(|d| d.as_secs()),
    });
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let (journal, venue) = (journal.clone(), venue.0.clone());
    rt.spawn(async move {
        let _ = journal
            .write(
                Level::Warn,
                "venue.book_events_lost",
                Some(&venue),
                msg,
                Some(data),
            )
            .await;
    });
}
