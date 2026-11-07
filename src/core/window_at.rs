//! Файл: core/window_at.rs
//! Что делает: решение 218 — стакан каждой биржи окна (решение 125) на момент решения о входе в сделку, по записанным
//! кадрам окна: последний ПОЛНЫЙ стакан строго до момента (кадры с временем получения ботом раньше него). Для пересчёта
//! старых записей «без потолка» (core/uncapped_recompute.rs).
//! Что считается полным стаканом, по порядку:
//! 1) стакан всей глубины по изменениям (потоки Diff и Snapshot — полный стакан плюс применённые изменения, правила
//!    версий window_book.rs), если он достоверен на момент решения (полный получен, разрывов нет);
//! 2) иначе — последний снимок N уровней торгового потока (поток Book, TapView::Book — тот стакан, по которому бот
//!    котировал; BingX `@depth<N>`, MEXC limit.depth), если после него до момента решения в этом потоке не было
//!    изменений, которые окно не применяет (TapView::Other — у Bybit так приходят изменения торгового стакана);
//! 3) иначе полного стакана нет — пересчёта нет, причина (например: у окна только изменения без снимка — Bybit без
//!    venue.window_depth_limit).
//!
//! Кадры разбирает адаптер своей площадки (контракт Venue::decode_tap; ядро форматов площадок не знает) — функцию
//! разбора передаёт вызывающий.
//! Кто вызывает: core/uncapped_recompute.rs, тесты (tests/uncapped_recompute.rs).
//! От чего зависит: window_book.rs, window_tap.rs (parse — строка файла → кадр), core/types, gateway/tap.rs.
//! Настройки: нет.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::core::types::{Asset, OrderBook, PriceLevel, TapDecoded, TapView, VenueId};
use crate::gateway::tap::{TapData, TapStream};

use super::window_book::Book;

/// Разбор кадра адаптером площадки: площадка, поток, содержимое → кадр в формате ядра или текст ошибки.
pub type Decode<'a> =
    &'a (dyn Fn(&VenueId, TapStream, &TapData) -> Result<TapDecoded, String> + Sync);

/// Стакан биржи на момент решения.
#[derive(Debug, Clone)]
pub struct BookAt {
    pub book: OrderBook,
    /// true — стакан всей глубины по изменениям; false — снимок N уровней торгового потока.
    pub full_depth: bool,
    /// Время получения ботом последнего кадра, вошедшего в стакан, Unix нс.
    pub recv_ns: i64,
}

impl BookAt {
    /// Текст для записи и отчёта: откуда стакан, уровни, время кадра. Ошибок нет.
    pub fn describe(&self) -> String {
        let what = if self.full_depth {
            "полный стакан по изменениям"
        } else {
            "снимок стакана торгового потока"
        };
        format!(
            "{} {}: {what}, {} bids / {} asks, кадр {}",
            self.book.venue.0,
            self.book.asset.ticker,
            self.book.bids.len(),
            self.book.asks.len(),
            ns_time(self.recv_ns).format("%H:%M:%S%.3f")
        )
    }
}

/// Состояние разбора одной биржи.
#[derive(Default)]
struct Venue {
    diff: Book,
    diff_ns: i64,
    /// Последний снимок торгового потока: (bids, asks, время) — None после неприменимого изменения.
    snap: Option<(Vec<PriceLevel>, Vec<PriceLevel>, i64)>,
    /// Счётчики для причины отказа: кадров стакана, изменений, сбоев разбора; почему снимок сброшен.
    book_frames: u64,
    diff_frames: u64,
    bad: u64,
    dropped: Option<String>,
}

/// Что делает: стаканы бирж окна на момент `before_ns` (см. шапку). Принимает: строки файла окна, биржи, монету,
/// момент (Unix нс, кадры строго раньше), разбор кадра. Возвращает: по каждой бирже — стакан или причину, почему
/// полного стакана нет. Ошибок нет.
pub fn books_before(
    rows: &[String],
    venues: &[VenueId],
    asset: &Asset,
    before_ns: i64,
    decode: Decode<'_>,
) -> HashMap<VenueId, Result<BookAt, String>> {
    let mut st: HashMap<VenueId, Venue> = venues
        .iter()
        .map(|v| (v.clone(), Venue::default()))
        .collect();
    for r in rows {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(r) else {
            continue;
        };
        let Some(f) = super::window_tap::parse(&v) else {
            continue;
        };
        if f.recv_ns >= before_ns || f.stream == TapStream::Trades {
            continue;
        }
        let Some(s) = st.get_mut(&f.venue) else {
            continue;
        };
        let d = match decode(&f.venue, f.stream, &f.data) {
            Ok(d) => d,
            Err(_) => {
                s.bad += 1;
                continue;
            }
        };
        match (f.stream, d.view) {
            (TapStream::Book, TapView::Book { bids, asks }) => {
                s.book_frames += 1;
                s.snap = Some((bids, asks, f.recv_ns));
                s.dropped = None;
            }
            (TapStream::Book, TapView::Other(t)) => {
                if s.snap.take().is_some() {
                    s.dropped = Some(format!("после снимка в потоке стакана пришло: {}", cut(&t)));
                }
            }
            (TapStream::Diff | TapStream::Snapshot, view @ TapView::Diff { .. }) => {
                s.diff_frames += 1;
                s.diff.apply(&view, 0);
                s.diff_ns = f.recv_ns;
            }
            _ => {}
        }
    }
    st.into_iter()
        .map(|(v, s)| {
            let r = pick(&v, asset, s);
            (v, r)
        })
        .collect()
}

/// Выбор полного стакана биржи по правилам шапки. Ошибки: текст — полного стакана нет.
fn pick(v: &VenueId, asset: &Asset, s: Venue) -> Result<BookAt, String> {
    let make = |bids, asks, ns, full_depth| BookAt {
        book: OrderBook {
            venue: v.clone(),
            asset: asset.clone(),
            bids,
            asks,
            exchange_time: None,
            received_at: ns_time(ns),
        },
        full_depth,
        recv_ns: ns,
    };
    if let Some((bids, asks)) = s.diff.levels() {
        return Ok(make(bids, asks, s.diff_ns, true));
    }
    if let Some((bids, asks, ns)) = s.snap {
        return Ok(make(bids, asks, ns, false));
    }
    let mut why = format!(
        "{}: полного стакана до момента решения нет (снимков торгового потока {}, кадров изменений {} — полного по \
         изменениям нет или разрыв версий",
        v.0, s.book_frames, s.diff_frames
    );
    if s.bad > 0 {
        why.push_str(&format!(", не разобрано кадров {}", s.bad));
    }
    if let Some(d) = s.dropped {
        why.push_str(&format!("; {d}"));
    }
    why.push(')');
    Err(why)
}

/// Время из Unix нс (вне диапазона — начало эпохи). Ошибок нет.
pub fn ns_time(ns: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_nanos(ns)
}

/// Начало текста для причины (не больше 80 символов). Ошибок нет.
fn cut(t: &str) -> String {
    t.chars().take(80).collect()
}
