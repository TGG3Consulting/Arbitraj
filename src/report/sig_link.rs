//! Файл: report/sig_link.rs
//! Что делает: решение 255 — главный лист «Сигналы и сделки»: строка на направление окна и привязка сделок.
//! Сигнал по коду — окно (решение 125, core/window.rs): открывается, когда направление монеты прошло расчёт прибыльности
//! (Verdict::Accept), держит все направления той же монеты, закрывается после window.min_hold_ms тишины и конца нашей
//! сделки. Строка — направление окна (монета + биржа покупки «Биржа 1» + биржа продажи «Биржа 2»); начало и конец —
//! окна (WindowInfo.started_at / ended_at; открытое — «открыто»).
//! Привязка сделки (правило):
//! 1) по коду: сделка в списке окна (WindowInfo.deals — window_close.rs deals: сделки той же монеты, начатые не раньше
//!    окна, пока оно открыто) → строка её направления в этом окне;
//! 2) иначе по монете + направлению + времени: окно той же монеты (адрес контракта, решение 211), где было направление
//!    сделки, и старт сделки в [начало окна, конец окна] (открытое — до момента выгрузки);
//! 3) иначе — своя строка «сделка без записанного окна» (окно не писалось или удалено уборкой) с причиной.
//!
//! Трейд на бирже 1 — покупка сделки (Deal.buy, время TradeResult.finished_at), на бирже 2 — продажи (Deal.sells; продажа
//! не на бирже 2 — перегон или ручная — помечена площадкой). Размер — количество монеты и сумма USDT (количество ×
//! средняя цена). Несколько сделок в одном направлении окна — построчно в ячейке, с id сделки.
//! Кто вызывает: report/sig_build.rs.
//! От чего зависит: core/signals_src.rs, core/window.rs, core/deal.rs, notify/human.rs (venue), sig_model.rs.
//! Настройки: нет.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::core::deal::Deal;
use crate::core::signals_src::SignalsSrc;
use crate::core::types::{Direction, TradeResult};
use crate::core::window::WindowInfo;
use crate::gateway::contract::VenueKind;
use crate::notify::human::venue;

use super::sig_model::{MainRow, num, ts};

/// Что делает: id окна каждой сделки по правилам 1–2 шапки. Принимает: исходные данные. Возвращает: id сделки → id окна
/// (сделки без окна в карте нет). Ошибок нет.
pub fn link(src: &SignalsSrc) -> HashMap<String, String> {
    let mut m: HashMap<String, String> = HashMap::new();
    for w in &src.windows {
        for d in &w.deals {
            m.entry(d.id.clone()).or_insert_with(|| w.id.clone());
        }
    }
    for d in &src.deals {
        if m.contains_key(&d.id) {
            continue;
        }
        let hit = src.windows.iter().find(|w| {
            w.directions.contains(&d.direction)
                && d.started_at >= w.started_at
                && d.started_at <= w.ended_at.unwrap_or(src.now)
        });
        if let Some(w) = hit {
            m.insert(d.id.clone(), w.id.clone());
        }
    }
    m
}

/// Что делает: строка главного листа направления окна (без сводок объёмных листов — их ставит sig_build.rs).
/// Принимает: данные, окно, направление, сделки этого направления в окне. Ошибок нет.
pub fn row(src: &SignalsSrc, w: &WindowInfo, d: &Direction, deals: &[&Deal]) -> MainRow {
    let mut r = head(src, d);
    r.win_start = ts(w.started_at);
    r.win_end = w.ended_at.map(ts).unwrap_or_else(|| "открыто".into());
    trades(&mut r, d, deals);
    r
}

/// Что делает: строка сделки без записанного окна (правило 3). Принимает: данные, сделку. Ошибок нет.
pub fn orphan(src: &SignalsSrc, deal: &Deal) -> MainRow {
    let mut r = head(src, &deal.direction);
    let why = match &src.windows_off {
        Some(off) => format!("нет данных: окно не записано — окна не пишутся сейчас ({off})"),
        None => "нет данных: окна этой сделки нет в базе (окна не писались в то время или удалены уборкой \
                 window.keep_days / window.max_mb)"
            .to_string(),
    };
    r.win_start = why.clone();
    r.win_end = why.clone();
    for c in [&mut r.ticks, &mut r.book, &mut r.liquidity] {
        *c = why.clone();
    }
    trades(&mut r, &deal.direction, &[deal]);
    r
}

/// Монета, контракт, биржи и их вид. Ошибок нет.
fn head(src: &SignalsSrc, d: &Direction) -> MainRow {
    let kind = |v: &str| match src.kinds.get(v) {
        Some(VenueKind::Cex) => "ЦЕКС".to_string(),
        Some(VenueKind::Dex) => "ДЕКС".to_string(),
        None => "нет данных: площадка не подключена сейчас (вид берётся из шлюза)".to_string(),
    };
    MainRow {
        coin: d.asset.ticker.clone(),
        contract: d
            .asset
            .bsc_contract
            .map(|a| format!("{a:#x}"))
            .unwrap_or_else(|| "нет: площадка не сообщила адрес контракта".into()),
        venue1: venue(&d.buy_on.0),
        venue2: venue(&d.sell_on.0),
        kind1: kind(&d.buy_on.0),
        kind2: kind(&d.sell_on.0),
        ..Default::default()
    }
}

/// Время и размер трейдов сделок (см. шапку). Сделок нет — «трейда не было». Ошибок нет.
fn trades(r: &mut MainRow, d: &Direction, deals: &[&Deal]) {
    if deals.is_empty() {
        let none = "трейда не было".to_string();
        (r.trade1_at, r.trade2_at, r.trade1_size, r.trade2_size) =
            (none.clone(), none.clone(), none.clone(), none);
        return;
    }
    let (mut t1, mut t2, mut s1, mut s2) = (vec![], vec![], vec![], vec![]);
    for deal in deals {
        let tag = format!("сделка {}", deal.id);
        match &deal.buy {
            Some(b) => {
                t1.push(format!("{} ({tag})", when(b, d, true)));
                s1.push(format!("{} ({tag})", size(b, &d.asset.ticker)));
            }
            None => {
                let why = format!("покупки нет, шаг {:?} ({tag})", deal.step);
                t1.push(why.clone());
                s1.push(why);
            }
        }
        if deal.sells.is_empty() {
            let why = format!("продажи нет, шаг {:?} ({tag})", deal.step);
            t2.push(why.clone());
            s2.push(why);
        }
        for s in &deal.sells {
            t2.push(format!("{} ({tag})", when(s, d, false)));
            s2.push(format!("{} ({tag})", size(s, &d.asset.ticker)));
        }
    }
    (r.trade1_at, r.trade2_at, r.trade1_size, r.trade2_size) =
        (t1.join("\n"), t2.join("\n"), s1.join("\n"), s2.join("\n"));
}

/// Время трейда; площадка не та, что в направлении, — с её именем. Ошибок нет.
fn when(t: &TradeResult, d: &Direction, buy: bool) -> String {
    let want = if buy { &d.buy_on } else { &d.sell_on };
    let at: DateTime<Utc> = t.finished_at;
    if &t.request.venue == want {
        ts(at)
    } else {
        format!("{} на {}", ts(at), venue(&t.request.venue.0))
    }
}

/// Размер трейда: количество монеты и сумма USDT, статус, если не исполнен полностью. Ошибок нет.
fn size(t: &TradeResult, ticker: &str) -> String {
    let usd = (t.filled_qty * t.avg_price).round_dp(4);
    let mut s = format!("{} {ticker} / {} USDT", num(t.filled_qty), num(usd));
    if !matches!(t.status, crate::core::types::TradeStatus::Filled) {
        s.push_str(&format!(" ({:?})", t.status));
    }
    s
}
