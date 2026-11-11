//! Файл: report/sig_frames.rs
//! Что делает: решение 255 — объёмные данные сигнала из файла окна (решение 125: <data-dir>/windows/<id>.jsonl.zst,
//! storage/episode_files.rs; строки — core/window_tap.rs line, window_worker.rs, window_dex.rs):
//! - кадры «Trades» (лента сделок биржи) → лист «Тики»: каждая сделка (цена, количество, сторона агрессора, id, время
//!   биржи и время получения ботом);
//! - кадры «Book» (снимок N уровней торгового потока — по нему бот котировал) → лист «Стаканы»: лучший bid / ask,
//!   уровней и сумма USDT по уровням кадра;
//! - кадры «Diff» / «Snapshot» (стакан всей глубины по изменениям) → «Стаканы»: тот же расчёт по стакану после кадра
//!   (core/window_book.rs: полный + изменения по правилам версий; недостоверный — с пометкой);
//! - строки «dex» (решение 241/245: точка цены DEX направления) → лист «Ликвидность пула»: цена, объём, блок, пулы
//!   маршрута; решение 255 — и состояние пулов маршрута (window.dex_pool_state).
//!
//! Кадры разбирает адаптер своей площадки (Venue::decode_tap) — ядро и отчёт форматов площадок не знают. Площадка не
//! подключена сейчас или кадр не разобран — счётчик и причина в заметках (сводка главной строки), кадр не теряется молча.
//! Чтение блокирующее (zstd) — вызывать в spawn_blocking.
//! Кто вызывает: report/sig_build.rs.
//! От чего зависит: core/signals_src.rs, core/window_tap.rs (parse), core/window_book.rs, core/window_dex.rs (DexPoint),
//! storage/episode_files.rs (each), sig_model.rs, sig_pool.rs.
//! Настройки: нет.

use std::collections::HashMap;

use crate::core::signals_src::SignalsSrc;
use crate::core::types::{Side, TapView};
use crate::core::window_at::ns_time;
use crate::core::window_book::Book;
use crate::core::window_dex::DexPoint;
use crate::gateway::tap::TapStream;
use crate::storage::episode_files;

use super::sig_model::{BookRow, LiqRow, Sig};
use super::sig_pool::pool_row;

/// Что не разобрано при чтении файла окна (для сводок главной строки).
#[derive(Debug, Default, Clone)]
pub struct Notes {
    /// Файл не прочитан — почему.
    pub file: Option<String>,
    /// По площадке: кадров не разобрано и последняя причина.
    pub undecoded: HashMap<String, (u64, String)>,
}

/// Что делает: читает файл окна и дописывает строки объёмных листов сигнала. Принимает: данные, сигнал (key — id
/// окна). Возвращает: заметки. Ошибок нет — в заметках.
pub fn fill(src: &SignalsSrc, sig: &mut Sig) -> Notes {
    let mut notes = Notes::default();
    let path = src.window_dir.join(format!("{}.jsonl.zst", sig.key));
    let mut books: HashMap<String, Book> = HashMap::new();
    let read = episode_files::each(&path, |line| frame(src, sig, &mut notes, &mut books, line));
    if let Err(e) = read {
        notes.file = Some(format!(
            "файл окна {} не прочитан: {e} (удалён уборкой window.keep_days / window.max_mb или не создан)",
            path.display()
        ));
    }
    notes
}

/// Одна строка файла окна → строки листов (см. шапку). Принимает: данные, сигнал, заметки, стаканы по изменениям,
/// строку. Ошибок нет — в заметках.
fn frame(
    src: &SignalsSrc,
    sig: &mut Sig,
    notes: &mut Notes,
    books: &mut HashMap<String, Book>,
    line: &str,
) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    if v["type"] == "dex" {
        if let Ok(p) = serde_json::from_value::<DexPoint>(v["point"].clone()) {
            dex(sig, &p);
        }
        return;
    }
    let Some(f) = crate::core::window_tap::parse(&v) else {
        return;
    };
    let d = match (src.decode)(&f.venue, f.stream, &f.data) {
        Ok(d) => d,
        Err(e) => {
            let u = notes.undecoded.entry(f.venue.0.clone()).or_default();
            (u.0, u.1) = (u.0 + 1, e);
            return;
        }
    };
    let (key, venue, at) = (sig.key.clone(), f.venue.0.clone(), ns_time(f.recv_ns));
    match (f.stream, &d.view) {
        (TapStream::Trades, TapView::Trades(ts)) => {
            sig.ticks.extend(ts.iter().map(|t| super::sig_model::Tick {
                key: key.clone(),
                venue: venue.clone(),
                ex_ms: t.time_ms,
                recv_ns: f.recv_ns,
                price: t.price,
                qty: t.qty,
                taker: t.taker.clone(),
                id: t.id.clone(),
            }))
        }
        (TapStream::Book, TapView::Book { bids, asks }) => {
            let side = |l: &[crate::core::types::PriceLevel]| -> rust_decimal::Decimal {
                l.iter().map(|x| x.price * x.qty).sum()
            };
            sig.books.push(BookRow {
                key,
                venue,
                source: "снимок потока стакана (по нему бот котировал)".into(),
                ex_ms: d.exchange_ms,
                at,
                bid: bids.iter().map(|l| l.price).max(),
                ask: asks.iter().map(|l| l.price).min(),
                levels: (bids.len(), asks.len()),
                usd: (side(bids), side(asks)),
                note: String::new(),
            });
        }
        (TapStream::Diff | TapStream::Snapshot, view @ TapView::Diff { .. }) => {
            let b = books.entry(venue.clone()).or_default();
            let note = b.apply(view, 0).map(|x| x.note).unwrap_or_default();
            let s = b.summary();
            let note = if s.ok {
                note
            } else {
                format!("недостоверен: {note}")
            };
            sig.books.push(BookRow {
                key,
                venue,
                source: "стакан всей глубины по изменениям".into(),
                ex_ms: d.exchange_ms,
                at,
                bid: s.bid,
                ask: s.ask,
                levels: s.levels,
                usd: s.usd,
                note,
            });
        }
        _ => {}
    }
}

/// Точка цены DEX и состояния пулов в ней → строки «Ликвидность пула». Ошибок нет.
fn dex(sig: &mut Sig, p: &DexPoint) {
    let dir = format!("{} → {}", p.buy_on.0, p.sell_on.0);
    let side = match p.side {
        Side::Buy => "покупка на DEX",
        Side::Sell => "продажа на DEX",
    };
    let size = match &p.deal {
        Some(s) => format!(
            "{side}, объём сделки {} ({}, запрошено {})",
            s.id, s.qty_from, s.asked
        ),
        None => format!("{side}, объём сравнения тика"),
    };
    sig.liq.push(LiqRow {
        key: sig.key.clone(),
        at: Some(p.at),
        source: "окно: точка цены DEX".into(),
        direction: dir.clone(),
        venue: p.venue.0.clone(),
        pool: p.pools.join(" | "),
        kind: String::new(),
        block: p.block,
        side: size,
        price: Some(p.price),
        qty: Some(p.qty),
        depth_usd: None,
        state: String::new(),
    });
    for st in &p.states {
        if let Ok(pool) = serde_json::from_value::<crate::core::types::Pool>(st.clone()) {
            sig.liq
                .push(pool_row(&sig.key, "окно: состояние пула", &dir, &pool));
        }
    }
}
