//! Файл: gateway/book_lost_log.rs
//! Что делает: решение 324 — запись журнала уровня Warn `venue.book_events_lost` о стаканах биржи, не вошедших в канал
//! событий ядра сразу (канал полон): «отложено, заменено более свежими, доставлено, потеряно, ждут» с прошлой записи —
//! не чаще раза в log.pool_events_lost_interval_s секунд (первая — сразу; та же настройка, что у пулов DEX — решение
//! 313, новых настроек журнала нет). Не задана — записей нет (счёт в наборе копится и уйдёт первой записью).
//! Пишется при каждом не сразу отправленном кадре (если пора) и когда общий набор опустел (крючок core_out.rs).
//! Ревью 324 (Б2): интервал — в памяти (Side::interval, gateway/setting_cell.rs); не задан — выход сразу после одной
//! атомарной загрузки (без замков, часов и Instant::now; потери копятся в Side::lost). Н7: запись о ПОТЕРЯХ
//! («потеряно N») появляется и без venue.book_events_keep, если задан интервал — изменение журнала, не торговли.
//! Кто вызывает: gateway/core_out_book.rs.
//! От чего зависит: gateway/core_out.rs (счёт класса), gateway/core_out_side.rs (интервал, потери, часы),
//! storage/journal.rs, tokio (запись отдельной задачей).
//! Настройки: log.pool_events_lost_interval_s (вся система или площадка; необязательная; читает core_out_book.rs).

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::core::types::VenueId;
use crate::storage::journal::{Journal, Level};

use super::core_out::Feed;
use super::core_out_item::Class;
use super::core_out_side::Side;
use super::keep_last::Counts;

/// Если пора — запись счёта класса «стаканы площадки» (счёт обнуляется). Принимает: набор, площадку, состояние класса,
/// журнал. Ошибок нет; нет рантайма tokio — записи нет.
pub fn note(feed: &Feed, venue: &VenueId, side: &Side, journal: &Journal) {
    let Some(every) = side.interval.as_ref().and_then(|c| c.get()) else {
        return;
    };
    let class = Class::Book(venue.clone());
    // Нечего сказать (всё уходило сразу) — часы не трогаются, записи нет.
    if side.lost.load(Ordering::Relaxed) == 0 && feed.peek(&class) == Counts::default() {
        return;
    }
    let due = side
        .clock
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .due(Instant::now(), || Some(Duration::from_secs(every)));
    let Some(since) = due else {
        return;
    };
    let c = feed.take(&class);
    let lost = side.lost.swap(0, Ordering::Relaxed);
    let window = since.map_or("первая сводка".to_string(), |d| {
        format!("с предыдущей сводки {} с", d.as_secs())
    });
    let mut msg = format!(
        "стаканы: канал событий ядра полон — отложено {} (venue.book_events_keep), заменено более свежими {}, \
         доставлено {}, потеряно {lost}, сейчас ждут ключей {}; {window}",
        c.deferred, c.replaced, c.delivered, c.waiting
    );
    if let Some(wait) = c.max_book_wait_ms {
        use std::fmt::Write;
        let _ = write!(msg, "; к передаче в канал: макс. ожидание ключа {wait} мс");
        if let Some(age) = c.max_book_age_ms {
            let _ = write!(msg, ", макс. возраст переданной книги {age} мс");
        }
    }
    let data = json!({
        "deferred": c.deferred, "replaced": c.replaced, "delivered": c.delivered, "lost": lost,
        "waiting": c.waiting, "since_s": since.map(|d| d.as_secs()),
        "max_book_wait_ms": c.max_book_wait_ms, "max_book_age_ms": c.max_book_age_ms,
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
