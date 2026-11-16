//! Файл: gateway/core_out_book.rs
//! Что делает: решение 324 — помощник адаптеров бирж: стакан (MarketEvent::Book) и «стакан недостоверен»
//! (MarketEvent::Invalidated) ядру через общий набор канала ядра (gateway/core_out.rs). Запросы и подписки к биржам не
//! меняются — только то, как уже собранный адаптером стакан отдаётся ядру.
//! - Стакан: без ожидания (как try_send). Канал полон: venue.book_events_keep не «да» (не задана) — как раньше, кадр
//!   потерян (Fed::Lost — адаптер считает свой прежний dropped); «да» — кадр не теряется: по ключу (площадка, тикер)
//!   в наборе ждёт ПОСЛЕДНИЙ кадр (стакан ядру — полный, адаптер сам собирает его из снимка и изменений: последний
//!   заменяет прежние) и уходит ядру, как только в канале появится место (общая очередь с пулами DEX). Время получения
//!   кадра (received_at) не подменяется — правила свежести ядра сами отбросят устаревшее. Ключ этой пары уже ждёт (в
//!   том числе «недостоверно») — кадр сливается с ждущим при любой настройке (иначе ядро получило бы кадры не по порядку).
//! - Invalidated: не теряется НИКОГДА (исправление: раньше `let _ = out.try_send` молча терял его при полном канале —
//!   ядро могло считать годным стакан, объявленный адаптером недостоверным). Без ожидания; полон — в набор срочным: в
//!   начало очереди, снимает ждавший старый кадр той же пары; кадр, пришедший после, уходит следом за ним.
//! - Журнал: venue.book_events_lost (gateway/book_lost_log.rs, интервал log.pool_events_lost_interval_s).
//!
//! Кто вызывает: ws_conn.rs и ws.rs (invalidate) бирж bingx, mexc, gate, kucoin, bybit, xt (ws_book.rs), bitget (и
//! снимки REST в mod.rs), binance, poloniex, okx, htx, kraken, coinex, phemex.
//! От чего зависит: gateway/core_out.rs, gateway/core_out_item.rs, gateway/book_lost_log.rs, settings/store.rs,
//! storage/journal.rs.
//! Настройки: venue.book_events_keep [вся система / площадка] (да/нет, необязательная; читается без текста ошибки,
//! только когда без неё не решить — канал полон или ждут другие); через book_lost_log.rs — log.pool_events_lost_interval_s.

use std::sync::Arc;

use tokio::sync::mpsc::Sender;

use crate::core::types::{OrderBook, VenueId};
use crate::settings::store::Store;
use crate::storage::journal::Journal;

use super::book_lost_log;
use super::core_out::{self, Feed};
use super::core_out_item::{Class, Item, Key};
use super::events::MarketEvent;
use super::keep_last::Offer;

/// Имя настройки решения 324.
pub const KEEP: &str = "venue.book_events_keep";

/// Чем кончилась отдача стакана (для адаптера).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fed {
    /// Ушёл в канал.
    Sent,
    /// Ждёт в наборе (не потерян).
    Kept,
    /// Канал полон, откладывать не велено — потерян (как раньше).
    Lost,
    /// Канал ядра закрыт.
    Closed,
}

/// Отдача стаканов одной биржи (на сессию соединения). Дешёво клонируется.
#[derive(Clone)]
pub struct BookOut {
    feed: Feed,
    venue: VenueId,
    settings: Store,
    journal: Journal,
}

impl BookOut {
    /// Помощник для канала ядра и площадки; ставит крючок записи журнала «набор опустел» класса стаканов площадки.
    /// Принимает: канал ядра, площадку, настройки, журнал. Ошибок нет.
    pub fn new(
        out: &Sender<MarketEvent>,
        venue: VenueId,
        settings: &Store,
        journal: &Journal,
    ) -> Self {
        let me = Self {
            feed: core_out::of(out),
            venue,
            settings: settings.clone(),
            journal: journal.clone(),
        };
        // Крючок не держит набор (получает его аргументом) — без кольцевой ссылки.
        let (v, s, j) = (me.venue.clone(), me.settings.clone(), me.journal.clone());
        let hook = move |f: &Feed| book_lost_log::note(f, &v, &s, &j);
        me.feed
            .on_drain(Class::Book(me.venue.clone()), Arc::new(hook));
        me
    }

    /// Стакан ядру (см. шапку). Принимает: канал ядра, стакан. Возвращает: Fed. Ошибок нет.
    pub fn book(&self, out: &Sender<MarketEvent>, book: OrderBook) -> Fed {
        let key = Key::Book(self.venue.clone(), book.asset.ticker.clone());
        let keep = || keep_on(&self.settings, &self.venue);
        let r = self
            .feed
            .offer(out, key, Item::Event(MarketEvent::Book(book), None), keep);
        match r {
            Offer::Sent => Fed::Sent,
            Offer::Closed => Fed::Closed,
            Offer::Deferred { .. } | Offer::Replaced => {
                self.note();
                Fed::Kept
            }
            Offer::Lost => {
                self.note();
                Fed::Lost
            }
        }
    }

    /// Запись журнала класса стаканов площадки, если пора (book_lost_log.rs). Ошибок нет.
    fn note(&self) {
        book_lost_log::note(&self.feed, &self.venue, &self.settings, &self.journal);
    }
}

/// «Стакан пары недостоверен» ядру — не теряется (см. шапку). Принимает: канал ядра, площадку, тикер монеты площадки.
/// Ошибок нет (канал закрыт — слать некому, как раньше).
pub fn invalidated(out: &Sender<MarketEvent>, venue: VenueId, ticker: &str) {
    let key = Key::Book(venue.clone(), ticker.to_string());
    let ev = MarketEvent::Invalidated {
        venue,
        ticker: ticker.to_string(),
        address: None,
    };
    // Срочное: настройка не читается, при полном канале — в начало очереди.
    let _ = core_out::of(out).offer(out, key, Item::Invalid(ev, None), || true);
}

/// venue.book_events_keep = да (не задана / нет / не да-нет — нет), без текста ошибки. Ошибок нет.
fn keep_on(settings: &Store, venue: &VenueId) -> bool {
    settings
        .resolve(KEEP, Some(&venue.0), None)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}
