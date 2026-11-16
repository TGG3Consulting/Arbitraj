//! Файл: gateway/core_out_must_deliver.rs
//! Что делает: решение 333, ревью — доставщик очереди «ждут места» одного канала событий ядра
//! (gateway/core_out_must_queue.rs): задача tokio, одна на канал. Ждёт место (Sender::reserve — без замка; честная
//! очередь tokio вместе с доставщиком набора keep_last), под замком снимает СНАЧАЛА Money, затем Info (ревью, Н1) и
//! отправляет тем же разрешением — между снятием и отправкой нет await. Очереди опустели — выходит; канал закрыт —
//! очищает их (счёт «сброшено»). Денежное событие ждало дольше venue.money_events_alert_ms — уведомление
//! venue.money_event_delayed в конец очереди Money (Н9). Guard Running (Н3, как keep_last_deliver.rs): паника или
//! снятие задачи — признак «доставщик работает» сбрасывается, следующее сообщение запустит нового.
//!
//! Кто вызывает: gateway/core_out_must.rs (MustOut::send — первое откладывание).
//! От чего зависит: gateway/core_out_must_queue.rs, gateway/core_out_must_log.rs, settings, storage/journal.rs, tokio.
//! Настройки: venue.money_events_alert_ms (через core_out_must_log.rs delayed); журнал — log.events_lost_interval_s.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tokio::sync::mpsc::Sender;

use crate::settings::store::Store;
use crate::storage::journal::Journal;

use super::core_out_must::Kind;
use super::core_out_must_log as log;
use super::core_out_must_queue::{Item, Queue, lock};
use super::events::MarketEvent;

/// Доставщик (см. шапку): ждёт место, отдаёт Money раньше Info; очереди опустели — выходит; канал закрыт — очищает.
/// Принимает: очередь, канал, настройки (порог задержки), журнал. Ошибок нет.
pub(super) async fn deliver(
    q: Arc<Queue>,
    out: Sender<MarketEvent>,
    settings: Store,
    journal: Journal,
) {
    let mut guard = Running {
        q: q.clone(),
        armed: true,
    };
    loop {
        let Ok(permit) = out.reserve().await else {
            let n = {
                let mut s = lock(&q.state);
                let n = s.money.len() + s.info.len();
                s.money.clear();
                s.info.clear();
                s.running = false;
                n
            };
            guard.armed = false;
            q.counts.dropped.fetch_add(n as u64, Ordering::Relaxed);
            log::note(&q, &settings, &journal);
            return;
        };
        let mut s = lock(&q.state);
        let (item, from_money) = match s.money.pop_front() {
            Some(i) => (i, true),
            None => match s.info.pop_front() {
                Some(i) => (i, false),
                None => {
                    s.running = false;
                    drop(s);
                    guard.armed = false;
                    log::note(&q, &settings, &journal);
                    return;
                }
            },
        };
        let waited = item.at.elapsed();
        // Уведомление о задержке — только для настоящего денежного события (не для самого уведомления о задержке).
        let alert = if from_money && Kind::of(&item.ev) == Kind::Money {
            log::delayed(&settings, &item.ev, waited)
        } else {
            None
        };
        permit.send(item.ev);
        let c = &q.counts;
        if from_money {
            c.delivered_money.fetch_add(1, Ordering::Relaxed);
            let ms = u64::try_from(waited.as_millis()).unwrap_or(u64::MAX);
            c.max_wait_money_ms.fetch_max(ms, Ordering::Relaxed);
        } else {
            c.delivered_info.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(ev) = alert {
            s.money.push_back(Item {
                ev,
                at: Instant::now(),
            });
        }
        // Опустело — выходим сразу (не ждём место ради пустой очереди).
        if s.money.is_empty() && s.info.is_empty() {
            s.running = false;
            drop(s);
            guard.armed = false;
            log::note(&q, &settings, &journal);
            return;
        }
    }
}

/// Признак «доставщик работает» сбрасывается, если задача кончилась не штатно (паника, снятие) — ревью 333, Н3.
struct Running {
    q: Arc<Queue>,
    armed: bool,
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.armed {
            lock(&self.q.state).running = false;
        }
    }
}
