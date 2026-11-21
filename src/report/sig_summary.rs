//! Файл: report/sig_summary.rs
//! Что делает: решение 255 — краткие сводки трёх объёмных колонок главной строки «Сигналы и сделки» («Данные по тикам»,
//! «Данные по стакану», «Данные по ликвидности в пуле за окно»): ключ сигнала (id окна), сколько записей, мин / макс
//! цены, глубина; нет записей — «нет данных» с причиной (файл окна не прочитан, запись на бирже не включилась, лента не
//! пишется, кадры не разобраны, у DEX нет ленты и стакана, точек DEX и состояний пулов нет и почему).
//! Кто вызывает: report/sig_build.rs.
//! От чего зависит: core/signals_src.rs, core/window.rs, sig_frames.rs (Notes), sig_model.rs, notify/human.rs (venue).
//! Настройки: нет (window.dex_pool_state, episode.* — только причины из SignalsSrc).

use rust_decimal::Decimal;

use crate::core::signals_src::SignalsSrc;
use crate::core::types::Direction;
use crate::core::window::WindowInfo;
use crate::gateway::contract::VenueKind;
use crate::notify::human::venue;

use super::sig_frames::Notes;
use super::sig_model::{MainRow, Sig, num};

/// Сводки трёх объёмных колонок главной строки направления. Ошибок нет.
#[allow(clippy::too_many_arguments)]
pub fn summaries(
    src: &SignalsSrc,
    w: &WindowInfo,
    d: &Direction,
    sig: &Sig,
    notes: &Notes,
    episodes: usize,
    ep_errs: &[String],
    r: &mut MainRow,
) {
    let key = format!("ключ {}", sig.key);
    let file = notes.file.clone();
    let mut ticks = vec![key.clone()];
    let mut book = vec![key.clone()];
    for v in [&d.buy_on, &d.sell_on] {
        let name = venue(&v.0);
        if src.kinds.get(&v.0) == Some(&VenueKind::Dex) {
            ticks.push(format!(
                "{name}: ленты сделок пула в окне нет (события Swap в окно не пишутся); цена DEX — лист «Ликвидность пула»"
            ));
            book.push(format!(
                "{name}: стакана нет (DEX) — пулы на листе «Ликвидность пула»"
            ));
            continue;
        }
        let why = |what: &str| -> String {
            if let Some(f) = &file {
                return format!("{name}: нет данных — {f}");
            }
            let mut s = format!("{name}: {what} за окно нет");
            match w
                .venues
                .iter()
                .find(|x| &x.venue == v)
                .and_then(|x| x.tap.clone())
            {
                Some(Err(e)) => s.push_str(&format!(" — запись окна на бирже не включилась: {e}")),
                Some(Ok(t)) if what == "сделок ленты" => {
                    if let Err(e) = t.trades_channel {
                        s.push_str(&format!(" — лента сделок не пишется: {e}"));
                    }
                }
                None => s.push_str(" — биржа не входила в запись окна"),
                _ => {}
            }
            if let Some((n, e)) = notes.undecoded.get(&v.0) {
                s.push_str(&format!("; не разобрано кадров {n}: {e}"));
            }
            s
        };
        let t: Vec<Decimal> = sig
            .ticks
            .iter()
            .filter(|x| x.venue == v.0)
            .map(|x| x.price)
            .collect();
        ticks.push(match range(&t) {
            Some((a, b)) => format!("{name}: сделок {}, цена {}–{}", t.len(), num(a), num(b)),
            None => why("сделок ленты"),
        });
        let rows: Vec<_> = sig.books.iter().filter(|x| x.venue == v.0).collect();
        let bids: Vec<Decimal> = rows.iter().filter_map(|x| x.bid).collect();
        let asks: Vec<Decimal> = rows.iter().filter_map(|x| x.ask).collect();
        let depth: Vec<Decimal> = rows.iter().map(|x| x.usd.0 + x.usd.1).collect();
        book.push(match (range(&bids), range(&asks), range(&depth)) {
            (Some(b), Some(a), Some(g)) => format!(
                "{name}: стаканов {}, bid {}–{}, ask {}–{}, глубина кадра {}–{} USDT",
                rows.len(),
                num(b.0),
                num(b.1),
                num(a.0),
                num(a.1),
                num(g.0.round_dp(2)),
                num(g.1.round_dp(2))
            ),
            _ if !rows.is_empty() => format!("{name}: стаканов {} (без уровней)", rows.len()),
            _ => why("стаканов"),
        });
    }
    r.ticks = ticks.join("; ");
    r.book = book.join("; ");
    r.liquidity = liq(src, d, sig, episodes, ep_errs, &key);
}

/// Сводка «Ликвидность пула» направления. Ошибок нет.
fn liq(
    src: &SignalsSrc,
    d: &Direction,
    sig: &Sig,
    episodes: usize,
    errs: &[String],
    key: &str,
) -> String {
    let dex = [&d.buy_on, &d.sell_on]
        .into_iter()
        .any(|v| src.kinds.get(&v.0) == Some(&VenueKind::Dex));
    if !dex {
        return format!("{key}; в направлении нет DEX — пула нет");
    }
    let dir = format!("{} → {}", d.buy_on.0, d.sell_on.0);
    let rows: Vec<_> = sig.liq.iter().filter(|x| x.direction == dir).collect();
    let count = |p: &str| rows.iter().filter(|x| x.source.contains(p)).count();
    let blocks: Vec<u64> = rows.iter().filter_map(|x| x.block).collect();
    let depth: Vec<Decimal> = rows.iter().filter_map(|x| x.depth_usd).collect();
    let mut s = vec![key.to_string()];
    s.push(format!(
        "точек цены DEX {}, состояний пулов {} (окно {}, эпизоды {}), глубин {}; эпизодов направления {episodes}",
        count("точка цены"),
        count("состояние пула"),
        count("окно: состояние"),
        rows.iter().filter(|x| x.source.starts_with("эпизод") && x.source.contains("состояние")).count(),
        count("глубина пула"),
    ));
    if let (Some(a), Some(b)) = (blocks.iter().min(), blocks.iter().max()) {
        s.push(format!("блоки {a}–{b}"));
    }
    if let Some((a, b)) = range(&depth) {
        s.push(format!(
            "глубина {}–{} USDT",
            num(a.round_dp(2)),
            num(b.round_dp(2))
        ));
    }
    if rows.is_empty() {
        s.push(
            "нет данных: точек цены DEX в окне нет (window.dex_price / window.dex_price_min_ms не были заданы) \
             и записей эпизодов нет"
                .into(),
        );
    }
    if count("окно: состояние") == 0 {
        let why = src
            .pool_state_off
            .clone()
            .unwrap_or_else(|| "включено сейчас, но в этом окне состояний нет".into());
        s.push(format!("состояние пулов в окне не писалось ({why})"));
    }
    if let Some(off) = &src.episodes_off {
        s.push(format!("эпизоды сейчас не пишутся: {off}"));
    }
    s.extend(errs.iter().cloned());
    s.join("; ")
}

/// Мин и макс. Ошибок нет: пусто — None.
fn range(v: &[Decimal]) -> Option<(Decimal, Decimal)> {
    Some((*v.iter().min()?, *v.iter().max()?))
}
