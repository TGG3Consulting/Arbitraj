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
//! - Ревью 324 (Б2): venue.book_events_keep и интервал журнала — в памяти (gateway/setting_cell.rs, перечитываются по
//!   рассылке изменений хранилища), одно состояние на класс «стаканы площадки» (Feed::side, core_out_side.rs): на кадр —
//!   без Store::resolve; потерянный кадр — атомарный счёт (Side::lost), при незаданном интервале — без замков и часов.
//!
//! Кто вызывает: ws_conn.rs и ws.rs (invalidate) бирж bingx, mexc, gate, kucoin, bybit, xt (ws_book.rs), bitget (и
//! снимки REST в mod.rs), binance, poloniex, okx, htx, kraken, coinex, phemex.
//! От чего зависит: gateway/core_out.rs, gateway/core_out_item.rs, gateway/book_lost_log.rs, settings/store.rs,
//! storage/journal.rs.
//! Настройки: venue.book_events_keep [вся система / площадка] (да/нет, необязательная; значение в памяти, нужно только
//! когда без него не решить — канал полон или ждут другие); log.pool_events_lost_interval_s [вся система / площадка]
//! (в памяти; запись — book_lost_log.rs).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use tokio::sync::mpsc::Sender;

use crate::core::types::{OrderBook, VenueId};
use crate::settings::store::Store;
use crate::storage::journal::Journal;

use super::book_lost_log;
use super::core_out::{self, Feed};
use super::core_out_item::{Class, Item, Key};
use super::core_out_side::Side;
use super::events::MarketEvent;
use super::keep_last::{Ask, Fast, Offer};
use super::setting_cell::{self, bool_of, u64_of};

/// Имя настройки решения 324.
pub const KEEP: &str = "venue.book_events_keep";

/// Интервал записи журнала (общий с пулами DEX, решение 313).
pub const INTERVAL: &str = "log.pool_events_lost_interval_s";

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
    /// Состояние класса «стаканы площадки» вне замка набора (ревью 324, Б2).
    side: Arc<Side>,
    journal: Journal,
}

impl BookOut {
    /// Помощник для канала ядра и площадки. Первый на класс создаёт его состояние (ячейки настроек — задачи tokio) и
    /// ставит крючок записи журнала «набор опустел» класса стаканов площадки. Принимает: канал ядра, площадку,
    /// настройки, журнал. Ошибок нет.
    pub fn new(
        out: &Sender<MarketEvent>,
        venue: VenueId,
        settings: &Store,
        journal: &Journal,
    ) -> Self {
        let feed = core_out::of(out);
        let class = Class::Book(venue.clone());
        let at = Some(venue.0.clone());
        let (side, created) = feed.side(&class, || {
            let keep = setting_cell::watch(settings, KEEP, at.clone(), bool_of);
            let every = setting_cell::watch(settings, INTERVAL, at, u64_of);
            Side::new(keep, Some(every))
        });
        if created {
            // Крючок не держит набор (получает его аргументом) — без кольцевой ссылки.
            let (v, sd, j) = (venue.clone(), side.clone(), journal.clone());
            let hook = move |f: &Feed| book_lost_log::note(f, &v, &sd, &j);
            feed.on_drain(class, Arc::new(hook));
        }
        Self {
            feed,
            venue,
            side,
            journal: journal.clone(),
        }
    }

    /// Стакан ядру (см. шапку). Принимает: канал ядра, стакан. Возвращает: Fed. Ошибок нет.
    pub fn book(&self, out: &Sender<MarketEvent>, book: OrderBook) -> Fed {
        let r = match self.feed.fast(out) {
            // Набор пуст (ревью 324, Б1): одна атомарная загрузка + try_reserve + send — без замка, ключа, настройки.
            Fast::Free(p) => {
                p.send(MarketEvent::Book(book));
                return Fed::Sent;
            }
            Fast::Closed => return Fed::Closed,
            Fast::Full if !self.side.keep.on() => Offer::Lost,
            Fast::Full | Fast::Busy => {
                // Медленный путь: ключ строится только здесь; настройка — из памяти, прочитана вне замка набора.
                let key = Key::Book(self.venue.clone(), book.asset.ticker.clone());
                let on = self.side.keep.on();
                let item = Item::Event(MarketEvent::Book(book));
                self.feed.slow(out, key, item, &mut Ask::new(move || on))
            }
        };
        match r {
            Offer::Sent => Fed::Sent,
            Offer::Closed => Fed::Closed,
            Offer::Deferred { .. } | Offer::Replaced { .. } => {
                self.note();
                Fed::Kept
            }
            Offer::Lost => {
                self.side.lost.fetch_add(1, Ordering::Relaxed);
                self.note();
                Fed::Lost
            }
        }
    }

    /// Запись журнала класса стаканов площадки, если пора (book_lost_log.rs). Ошибок нет.
    fn note(&self) {
        book_lost_log::note(&self.feed, &self.venue, &self.side, &self.journal);
    }
}

/// «Стакан пары недостоверен» ядру — не теряется (см. шапку). Принимает: канал ядра, площадку, тикер монеты площадки.
/// Ошибок нет (канал закрыт — слать некому, как раньше).
pub fn invalidated(out: &Sender<MarketEvent>, venue: VenueId, ticker: &str) {
    let feed = core_out::of(out);
    let ev = MarketEvent::Invalidated {
        venue: venue.clone(),
        ticker: ticker.to_string(),
        address: None,
    };
    match feed.fast(out) {
        Fast::Free(p) => p.send(ev),
        Fast::Closed => {}
        // Срочное: настройка не читается, при полном канале — в начало очереди.
        Fast::Full | Fast::Busy => {
            let key = Key::Book(venue, ticker.to_string());
            let _ = feed.slow(out, key, Item::Invalid(ev, None), &mut Ask::new(|| true));
        }
    }
}
