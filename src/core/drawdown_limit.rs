//! Файл: core/drawdown_limit.rs
//! Что делает: решение 414 — проверка просадки направления от пика при каждом пересчёте итога дня (daily_commit.rs):
//! - advance — изменения итога пары после слепка копятся в теоретическом балансе, пик поднимается (drawdown.rs);
//! - check — просадка БОЛЬШЕ предела (меньшее из risk.drawdown_pct × пик и risk.drawdown_max_usd) — пауза только этого
//!   направления существующим механизмом Risk.pair_paused решения 258 (идущие доводятся, новые не начинаются; у
//!   «DEX ↔ DEX» — пауза циклов), отметка Risk.draw_paused; уведомление risk.venue_daily_loss с цифрами (слепок, пик,
//!   баланс, просадка, предел — % и USDT). Повторная пауза после «Снять паузу» без нового слепка — только если
//!   просадка выросла (как у старого лимита, Risk.draw_seen).
//!   Пара без заданных risk.drawdown_* — по старому лимиту дня (daily_venue_limit.rs), здесь не трогается. Пара с
//!   заданными, но без слепка — проверки нет (торговля направлением не включается — risk_ready.rs).
//!   Снимают паузу: новый слепок («Сбросить лимит» направления / площадки / общий, новый день UTC — drawdown_snap.rs),
//!   «Снять паузу» направления или площадки (risk.rs).
//! Кто вызывает: daily_commit.rs (advance, check), risk.rs (snapshot — view_of), day_roll.rs.
//! От чего зависит: drawdown.rs, drawdown_cfg.rs, daily.rs (Book), day_view.rs, day_pair.rs (label), notify/human.rs.
//! Настройки: risk.drawdown_pct, risk.drawdown_max_usd (через drawdown_cfg.rs).

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::notify::human::{self as h, Msg, usd};

use super::daily::Book;
use super::day_pair::label;
use super::day_view::{DayView, PairDay};
use super::drawdown::{DrawView, SnapWay};
use super::engine::{Core, lk};

/// Что делает: учёт итога дня пар в их состоянии просадки (drawdown::advance). Принимает: книгу, итог пар, день,
/// плановые затраты возврата пар. Ошибок нет.
pub(super) fn advance(
    book: &mut Book,
    pairs: &BTreeMap<String, PairDay>,
    date: NaiveDate,
    pending: &BTreeMap<String, Decimal>,
) {
    for (p, st) in book.draw.iter_mut() {
        let r = pairs.get(p).map_or(Decimal::ZERO, |x| x.result_usd);
        let c = pending.get(p).copied().unwrap_or_default();
        super::drawdown::advance(st, r, date, c);
    }
}

/// Что делает: просадка пары по текущим настройкам. Принимает: ядро, id пары, итог дня в памяти. Возвращает: None —
/// пара по старому лимиту или слепка ещё не было. Ошибок нет.
pub fn view_of(core: &Core, p: &str, day: &DayView) -> Option<DrawView> {
    let l = super::drawdown_cfg::limit(core, p)?;
    let st = day.draw.get(p)?;
    let c = day.draw_pending.get(p).copied().unwrap_or_default();
    Some(super::drawdown::view(st, c, l))
}

/// Как снят слепок — словами. Ошибок нет.
pub fn way_text(w: SnapWay) -> &'static str {
    match w {
        SnapWay::Clean => "чистый — идущих сделок направления не было",
        SnapWay::Ledger => "по учёту — за 2 минуты сделки направления не кончились",
    }
}

/// Что делает: причина паузы с цифрами: «BingX → DEX: просадка 35 USDT больше предела 30 USDT (5 % от пика 1000 =
/// 50 USDT; не более 30 USDT — сработал USDT); слепок 1000 USDT (…, 00:00 UTC), пик 1000, баланс 965».
/// Принимает: id пары, вид, связку («больше предела», «из предела»). Ошибок нет.
pub fn describe(p: &str, v: &DrawView, rel: &str) -> String {
    let pct = match (v.pct, v.pct_usd) {
        (Some(x), Some(u)) => format!("{x} % от пика = {}", usd(u)),
        _ => "% не задан".into(),
    };
    let cap = v.usd.map_or("предел в USDT не задан".into(), |u| {
        format!("не более {}", usd(u))
    });
    let which = if v.by == "usd" { "USDT" } else { "%" };
    format!(
        "{}: просадка от пика {} {rel} {} ({pct}; {cap} — сработал {which}); слепок {} ({}, {} UTC), пик {}, \
         теоретический баланс {}",
        label(p),
        usd(v.drawdown_usd),
        usd(v.limit_usd),
        usd(v.snap.base_usd),
        way_text(v.snap.way),
        v.snap.at.format("%d.%m %H:%M"),
        usd(v.peak_usd),
        usd(v.balance_usd)
    )
}

/// Что делает: проверка просадки пар после пересчёта; превышена — пауза пары.
/// Принимает: ядро. Возвращает: (пары, поставленные на паузу сейчас, с причиной; тексты «просадка сейчас в пределе»).
/// Ошибок нет. Замок: risk (настройки читаются под ним, как у старого лимита).
pub(super) fn check(core: &Core) -> (Vec<(String, String)>, Vec<String>) {
    let mut paused = Vec::new();
    let mut cleared = Vec::new();
    let mut r = lk(&core.inner.risk);
    let names: Vec<String> = r.day.pairs.keys().cloned().collect();
    for p in names {
        if !super::drawdown_cfg::active(core, &p) {
            // Пределы просадки убрали — пауза по просадке снимается (дальше — старый лимит дня этой пары).
            if r.draw_paused.remove(&p) && r.pair_paused.remove(&p).is_some() {
                cleared.push(format!(
                    "{}: пределы просадки risk.drawdown_* больше не заданы — пауза по просадке снята, направление \
                     считается по старому лимиту дня (risk.max_daily_loss_usd)",
                    label(&p)
                ));
            }
            continue;
        }
        let v = view_of(core, &p, &r.day);
        let was = r.pair_paused.contains_key(&p);
        let seen = r.draw_seen.get(&p).copied();
        if let Some(v) = &v {
            r.draw_seen.insert(p.clone(), v.drawdown_usd);
            // Пауза — при переходе через предел или при росте просадки (после «Снять паузу» без нового слепка).
            let grew = seen.is_none_or(|s| s <= v.limit_usd || v.drawdown_usd > s);
            if v.exceeded && grew && !was {
                let t = describe(&p, v, "больше предела");
                r.pair_paused.insert(p.clone(), t.clone());
                r.draw_paused.insert(p.clone());
                paused.push((p.clone(), t));
            } else if !v.exceeded
                && was
                && r.draw_paused.contains(&p)
                && seen.is_some_and(|s| s > v.limit_usd)
            {
                cleared.push(format!(
                    "{}. Пауза направления снимется с новым слепком: «Сбросить лимит» направления или новый день \
                     (UTC) — или «Снять паузу»",
                    describe(&p, v, "сейчас в пределе")
                ));
            }
        }
        let now = r.pair_paused.contains_key(&p);
        if let Some(e) = r.day.pairs.get_mut(&p) {
            e.draw_on = true;
            e.limit = None;
            e.draw = v;
            e.paused = now;
        }
    }
    (paused, cleared)
}

/// Что делает: текст Telegram о паузе направления по просадке (решение 164: коротко, с цифрами). Принимает: id пары,
/// причину. Ошибок нет.
pub fn stop_text(p: &str, why: &str) -> String {
    let cycles = super::day_pair::venue_of(p).is_none();
    let head = if cycles {
        "Циклы DEX ↔ DEX на паузе: просадка от пика".to_string()
    } else {
        format!("Направление {} на паузе: просадка от пика", label(p))
    };
    Msg::new(h::STOP, head)
        .line(why)
        .line("Другие направления торгуют; идущие сделки доводятся")
        .line("Снимется с новым слепком баланса — новый день (UTC) или «Сбросить лимит»")
        .todo(format!(
            "проверить сделки направления, затем «Сбросить лимит дня» или «Снять паузу» по {}",
            label(p)
        ))
        .render()
}
