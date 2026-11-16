//! Файл: gateway/core_out_must_log.rs
//! Что делает: решение 333, ревью — журнал и уведомление очереди «ждут места» сообщений адаптеров ядру
//! (gateway/core_out_must_queue.rs):
//! - Н2: запись `venue.core_events_queue` (Warn) — отложено / доставлено / потеряно / сброшено с прошлой записи, дольше
//!   всех ждавшее денежное и сколько ждут сейчас. Пишется при откладывании и когда очередь опустела: не чаще раза в
//!   log.events_lost_interval_s (первая — сразу); не задана — на 1-м, 2-м, 4-м, 8-м… поводе (шквал — несколько строк).
//! - Н8: запись о потере `venue.core_events_lost` (gateway/lost_log.rs) — с площадкой события и описанием.
//! - Н9: денежное событие ждало места дольше venue.money_events_alert_ms — уведомление `venue.money_event_delayed`
//!   (MarketEvent::Notice ядру → Telegram по notify.event.*). Не задана — уведомления нет.
//!
//! Кто вызывает: gateway/core_out_must.rs, gateway/core_out_must_queue.rs.
//! От чего зависит: gateway/lost_log.rs, gateway/log_clock.rs, settings, storage/journal.rs, tokio (запись отдельной
//! задачей — вызывающий не ждёт).
//! Настройки: log.events_lost_interval_s, venue.money_events_alert_ms (вся система; необязательные).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::core::types::VenueId;
use crate::settings::read::At;
use crate::settings::store::Store;
use crate::storage::journal::{Journal, Level};

use super::core_out_must::Kind;
use super::core_out_must_queue::{Queue, Why};
use super::events::MarketEvent;
use super::log_clock::Clock;
use super::lost_log::{INTERVAL, Note};

/// Вид записи журнала о потерях.
pub const LOST_KIND: &str = "venue.core_events_lost";
/// Вид записи журнала о счёте очереди (Н2).
pub const QUEUE_KIND: &str = "venue.core_events_queue";
/// Порог задержки денежного события для уведомления, мс (Н9).
pub const ALERT_MS: &str = "venue.money_events_alert_ms";
/// Код уведомления о задержке (notify/catalog.rs).
pub const DELAYED_CODE: &str = "venue.money_event_delayed";

/// Часы и номер повода записи счёта очереди.
#[derive(Debug, Default)]
pub struct Notes {
    n: AtomicU64,
    clock: Mutex<Clock>,
}

/// Площадка события (None — у события её нет). Ошибок нет.
pub fn venue_of(ev: &MarketEvent) -> Option<&VenueId> {
    match ev {
        MarketEvent::Notice { venue, .. }
        | MarketEvent::Invalidated { venue, .. }
        | MarketEvent::SpotPairs { venue, .. }
        | MarketEvent::CheckPass { venue, .. }
        | MarketEvent::PoolsLoading { venue, .. } => Some(venue),
        MarketEvent::WalletQueue(w) => Some(&w.venue),
        MarketEvent::ExitGap(g) => Some(&g.venue),
        MarketEvent::CoinCheck(c) => Some(&c.venue),
        MarketEvent::PoolCheck(c) => Some(&c.venue),
        MarketEvent::Balance(b) => Some(&b.venue),
        MarketEvent::Book(_) | MarketEvent::Pool(_) | MarketEvent::SendChannel(_) => None,
    }
}

/// Короткое описание события для журнала. Ошибок нет.
pub fn describe(ev: &MarketEvent) -> String {
    match ev {
        MarketEvent::Notice { code, .. } => format!("уведомление {code}"),
        MarketEvent::WalletQueue(w) => format!("очередь кошелька {}: {:?}", w.venue.0, w.state),
        MarketEvent::ExitGap(g) => {
            format!("замер выхода {} (сделка {})", g.asset.ticker, g.deal_id)
        }
        MarketEvent::SendChannel(c) => format!("счётчики канала отправки {}", c.id),
        MarketEvent::CoinCheck(c) => format!("проверка монеты {}", c.asset.ticker),
        MarketEvent::Balance(b) => format!("изменение баланса {} {}", b.venue.0, b.asset.ticker),
        MarketEvent::Invalidated { venue, ticker, .. } => {
            format!("недостоверно {} {ticker}", venue.0)
        }
        _ => String::new(),
    }
}

/// Потеря сообщения: счёт и запись venue.core_events_lost (Н8 — с площадкой). Принимает: очередь, настройки, журнал,
/// род, почему, сообщение. Ошибок нет.
pub fn lost(
    q: &Queue,
    settings: &Store,
    journal: &Journal,
    kind: Kind,
    why: Why,
    ev: &MarketEvent,
) {
    let what = match why {
        Why::Max(_) => {
            "канал событий ядра полон — уведомление / счётчик потеряны: очередь достигла предела venue.info_events_max"
        }
        Why::NoRuntime => {
            "канал событий ядра полон, доставщика запустить нельзя (нет рантайма tokio) — сообщение потеряно"
        }
    };
    let counter = match kind {
        Kind::Money => &q.lost_money,
        Kind::Info => {
            q.counts.lost_info.fetch_add(1, Ordering::Relaxed);
            &q.lost_info
        }
    };
    let venue = venue_of(ev).map(|v| v.0.clone());
    let note = Note {
        kind: LOST_KIND,
        venue: venue.as_deref(),
        what,
        last: describe(ev),
    };
    counter.hit(settings, journal, note);
}

/// Н9: уведомление о задержке денежного события (None — порог не задан, не превышен или у события нет площадки).
/// Принимает: настройки, событие, сколько ждало. Ошибок нет.
pub fn delayed(settings: &Store, ev: &MarketEvent, waited: Duration) -> Option<MarketEvent> {
    let max = settings
        .integer(ALERT_MS, At::default())
        .ok()
        .filter(|ms| *ms > 0)?;
    let ms = u64::try_from(waited.as_millis()).unwrap_or(u64::MAX);
    if ms < max {
        return None;
    }
    let venue = venue_of(ev)?.clone();
    let text = format!(
        "Денежное событие ждало места в канале событий ядра {ms} мс (порог {ALERT_MS} = {max}): {}. До доставки ядро \
         о нём не знало и решало по прежнему состоянию.",
        describe(ev)
    );
    Some(MarketEvent::Notice {
        venue,
        code: DELAYED_CODE.to_string(),
        text,
    })
}

/// Н2: если пора — запись счёта очереди (счёт с прошлой записи обнуляется). Принимает: очередь, настройки, журнал.
/// Ошибок нет; нет рантайма tokio — записи нет.
pub fn note(q: &Queue, settings: &Store, journal: &Journal) {
    let n = q.notes.n.fetch_add(1, Ordering::Relaxed).saturating_add(1);
    let every = settings
        .integer(INTERVAL, At::default())
        .ok()
        .filter(|s| *s > 0);
    let due: Option<Option<Duration>> = match every {
        Some(e) => q
            .notes
            .clock
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .due(Instant::now(), || Some(Duration::from_secs(e))),
        None => n.is_power_of_two().then_some(None),
    };
    let Some(window) = due else {
        return;
    };
    let c = &q.counts;
    let take = |a: &AtomicU64| a.swap(0, Ordering::Relaxed);
    let (dm, di) = (take(&c.deferred_money), take(&c.deferred_info));
    let (vm, vi) = (take(&c.delivered_money), take(&c.delivered_info));
    let (li, dr, w) = (
        take(&c.lost_info),
        take(&c.dropped),
        take(&c.max_wait_money_ms),
    );
    let now = q.snapshot();
    let when = window.map_or("с прошлой записи".to_string(), |d| {
        format!("за {} с", d.as_secs())
    });
    let msg = format!(
        "канал событий ядра был полон: отложено денежных {dm}, уведомлений {di}; доставлено денежных {vm}, уведомлений \
         {vi}; потеряно уведомлений {li}; сброшено (канал закрыт) {dr}; дольше всех ждало денежное {w} мс; ждут сейчас \
         денежных {}, уведомлений {} ({when})",
        now.money, now.info
    );
    let data = json!({
        "deferred_money": dm, "deferred_info": di, "delivered_money": vm, "delivered_info": vi, "lost_info": li,
        "dropped": dr, "max_wait_money_ms": w, "waiting_money": now.money, "waiting_info": now.info,
        "since_s": window.map(|d| d.as_secs()),
    });
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let journal = journal.clone();
    rt.spawn(async move {
        let _ = journal
            .write(Level::Warn, QUEUE_KIND, None, msg, Some(data))
            .await;
    });
}
