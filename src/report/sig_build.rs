//! Файл: report/sig_build.rs
//! Что делает: решение 255 — сборка данных книги «Сигналы и сделки» за период [from, to) (по началу окна; сделка без
//! окна — по старту сделки): сигналы-окна (sig_link.rs — строки и сделки), объёмные листы из файла окна (sig_frames.rs)
//! и эпизодов направления (sig_episodes.rs), краткие сводки в главной строке (sig_summary.rs). Блокирующее (чтение
//! файлов) — вызывать в spawn_blocking.
//! Кто вызывает: report/sig_stream.rs, тесты.
//! От чего зависит: core/signals_src.rs, sig_link.rs, sig_frames.rs, sig_episodes.rs, sig_summary.rs, sig_model.rs.
//! Настройки: нет.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::core::deal::Deal;
use crate::core::signals_src::SignalsSrc;
use crate::core::window::WindowInfo;

use super::in_period;
use super::sig_model::Sig;

/// Сигнал до чтения файлов: окно или сделка без окна.
pub enum Item<'a> {
    Window(&'a WindowInfo),
    Orphan(&'a Deal),
}

/// Что делает: сигналы периода без чтения файлов, старые сверху (окна — по началу, сделки без окна — по старту).
/// Принимает: данные, привязку сделок (sig_link.rs link), границы. Возвращает: (время, сигнал). Ошибок нет.
pub fn items<'a>(
    src: &'a SignalsSrc,
    links: &HashMap<String, String>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> Vec<(DateTime<Utc>, Item<'a>)> {
    let mut out: Vec<(DateTime<Utc>, Item<'a>)> = src
        .windows
        .iter()
        .filter(|w| in_period(w.started_at, from, to))
        .map(|w| (w.started_at, Item::Window(w)))
        .collect();
    out.extend(
        src.deals
            .iter()
            .filter(|d| !links.contains_key(&d.id) && in_period(d.started_at, from, to))
            .map(|d| (d.started_at, Item::Orphan(d))),
    );
    out.sort_by_key(|x| x.0);
    out
}

/// Что делает: сигнал с объёмными строками — читает файл окна и эпизоды направлений (блокирующее). Принимает: данные,
/// привязку сделок, сигнал. Возвращает: Sig. Ошибок нет — причины в сводках.
pub fn make(src: &SignalsSrc, links: &HashMap<String, String>, item: &Item<'_>) -> Sig {
    let w = match item {
        Item::Orphan(d) => {
            return Sig {
                key: format!("сделка {}", d.id),
                at: d.started_at,
                rows: vec![super::sig_link::orphan(src, d)],
                ..Default::default()
            };
        }
        Item::Window(w) => *w,
    };
    let mut sig = Sig {
        key: w.id.clone(),
        at: w.started_at,
        ..Default::default()
    };
    let notes = super::sig_frames::fill(src, &mut sig);
    let have: HashSet<String> = sig.books.iter().map(|b| b.venue.clone()).collect();
    for d in &w.directions {
        let eps = super::sig_episodes::of(src, w, d);
        let errs = super::sig_episodes::fill(src, &mut sig, &eps, d, &have);
        let deals: Vec<&Deal> = src
            .deals
            .iter()
            .filter(|x| &x.direction == d && links.get(&x.id) == Some(&w.id))
            .collect();
        let mut r = super::sig_link::row(src, w, d, &deals);
        super::sig_summary::summaries(src, w, d, &sig, &notes, eps.len(), &errs, &mut r);
        sig.rows.push(r);
    }
    sig
}

/// Что делает: все сигналы периода целиком в памяти (тесты; отчёт идёт потоком — sig_stream.rs).
/// Принимает: данные, границы. Возвращает: сигналы, старые сверху. Ошибок нет.
pub fn build(src: &SignalsSrc, from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> Vec<Sig> {
    let links = super::sig_link::link(src);
    items(src, &links, from, to)
        .iter()
        .map(|(_, i)| make(src, &links, i))
        .collect()
}
