//! Файл: report/sig_episodes.rs
//! Что делает: решение 255 — данные сигнала из эпизодов (решение 74: <data-dir>/episodes/<id>.jsonl.zst, строки «calc» —
//! core/episode_snap.rs record) того же направления, что пересекаются с окном по времени: на каждый записанный пересчёт
//! (не чаще episode.min_interval_ms, пока направление проходит расчёт) —
//! - состояние пулов маршрута (Pool: резервы / sqrtPrice / ликвидность, блок) → лист «Ликвидность пула»;
//! - глубина пулов по стороне сделки в полосе dex.depth_band_pct из карточки (Card.pools) → «Ликвидность пула»;
//! - верхние episode.book_levels уровней стакана биржи → лист «Стаканы», только для биржи, у которой в файле окна
//!   стакана нет (запись окна на ней не включилась) — иначе это повтор кадров окна.
//!
//! Чтение блокирующее (zstd) — вызывать в spawn_blocking.
//! Кто вызывает: report/sig_build.rs.
//! От чего зависит: core/signals_src.rs, core/episode.rs (EpisodeInfo), core/types (Pool, OrderBook),
//! storage/episode_files.rs (each), sig_model.rs, sig_pool.rs.
//! Настройки: нет.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::episode::EpisodeInfo;
use crate::core::signals_src::SignalsSrc;
use crate::core::types::{Direction, OrderBook, Pool};
use crate::core::window::WindowInfo;
use crate::storage::episode_files;

use super::sig_model::{BookRow, LiqRow, Sig};
use super::sig_pool::{pool_row, time_of};

/// Эпизоды направления, пересекающиеся с окном по времени. Ошибок нет.
pub fn of<'a>(src: &'a SignalsSrc, w: &WindowInfo, d: &Direction) -> Vec<&'a EpisodeInfo> {
    let w_end = w.ended_at.unwrap_or(src.now);
    src.episodes
        .iter()
        .filter(|e| &e.direction == d)
        .filter(|e| e.started_at <= w_end && e.ended_at.unwrap_or(src.now) >= w.started_at)
        .collect()
}

/// Что делает: дописывает строки эпизодов в сигнал. Принимает: данные, сигнал, эпизоды направления, направление,
/// биржи, у которых в окне уже есть стакан. Возвращает: причины — файлы эпизодов не прочитаны. Ошибок нет.
pub fn fill(
    src: &SignalsSrc,
    sig: &mut Sig,
    eps: &[&EpisodeInfo],
    d: &Direction,
    have_book: &HashSet<String>,
) -> Vec<String> {
    let dir = format!("{} → {}", d.buy_on.0, d.sell_on.0);
    let mut errs = vec![];
    for e in eps {
        let path = src.episode_dir.join(format!("{}.jsonl.zst", e.id));
        let r = episode_files::each(&path, |line| {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                return;
            };
            if v["type"] == "calc" {
                calc(sig, &e.id, &dir, &v, have_book);
            }
        });
        if let Err(err) = r {
            errs.push(format!(
                "эпизод {}: файл не прочитан: {err} (удалён уборкой episode.keep_days / episode.max_mb)",
                e.id
            ));
        }
    }
    errs
}

/// Одна запись пересчёта эпизода → строки листов. Ошибок нет: нечитаемые части пропускаются.
fn calc(sig: &mut Sig, ep: &str, dir: &str, v: &Value, have_book: &HashSet<String>) {
    let at = time_of(&v["t"]);
    let src = format!("эпизод {ep}: состояние пула");
    for p in v["pools"].as_array().into_iter().flatten() {
        if let Ok(p) = serde_json::from_value::<Pool>(p.clone()) {
            sig.liq.push(pool_row(&sig.key, &src, dir, &p));
        }
    }
    for p in v["card"]["pools"].as_array().into_iter().flatten() {
        sig.liq.push(depth(sig, ep, dir, at, p));
    }
    for b in v["books"].as_array().into_iter().flatten() {
        let Ok(b) = serde_json::from_value::<OrderBook>(b.clone()) else {
            continue;
        };
        if have_book.contains(&b.venue.0) {
            continue;
        }
        let usd = |l: &[crate::core::types::PriceLevel]| -> Decimal {
            l.iter().map(|x| x.price * x.qty).sum()
        };
        sig.books.push(BookRow {
            key: sig.key.clone(),
            venue: b.venue.0.clone(),
            source: format!("эпизод {ep}: верхние уровни (episode.book_levels) на пересчёте"),
            ex_ms: b.exchange_time.map(|t| t.timestamp_millis()),
            at: b.received_at,
            bid: b.bids.iter().map(|l| l.price).max(),
            ask: b.asks.iter().map(|l| l.price).min(),
            levels: (b.bids.len(), b.asks.len()),
            usd: (usd(&b.bids), usd(&b.asks)),
            note: String::new(),
        });
    }
}

/// Глубина пула из карточки эпизода. Ошибок нет.
fn depth(sig: &Sig, ep: &str, dir: &str, at: Option<DateTime<Utc>>, p: &Value) -> LiqRow {
    let dec = |k: &str| serde_json::from_value::<Decimal>(p[k].clone()).ok();
    let side = match p["side"].as_str() {
        Some("Buy") => "покупка на DEX",
        Some("Sell") => "продажа на DEX",
        _ => "",
    };
    let band = dec("band_pct")
        .map(|b| format!(", полоса {b}%"))
        .unwrap_or_default();
    LiqRow {
        key: sig.key.clone(),
        at,
        source: format!("эпизод {ep}: глубина пула"),
        direction: dir.into(),
        venue: p["venue"].as_str().unwrap_or_default().into(),
        pool: p["pool"].as_str().unwrap_or_default().into(),
        kind: format!(
            "{} {}",
            p["kind"].as_str().unwrap_or_default(),
            p["family"].as_str().unwrap_or_default()
        ),
        block: p["block"].as_u64(),
        side: format!("{side}{band}"),
        depth_usd: dec("depth_usd"),
        state: p["error"]
            .as_str()
            .map(|e| format!("глубины нет: {e}"))
            .unwrap_or_default(),
        ..Default::default()
    }
}
