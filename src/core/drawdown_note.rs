//! Файл: core/drawdown_note.rs
//! Что делает: решение 414 — тексты снятия слепка баланса направления (вынесено из drawdown_snap.rs): снят — журнал
//! core.drawdown_snap (факт, своё в пути, чужое на счёте с ID сделок, итог, какая пауза снята), Telegram
//! risk.daily_limit_reset, если запросил человек или снята пауза; не снят — причина в запросе (экран, Want.note) и
//! журнал при смене причины.
//! Ещё (повторное ревью, вынесено из drawdown_limit.rs): как снят слепок словами, причина паузы по просадке с цифрами,
//! текст Telegram о паузе.
//! Кто вызывает: drawdown_snap.rs (apply, take), drawdown_limit.rs (describe), daily_commit.rs (stop_text).
//! От чего зависит: drawdown.rs (Snap, DrawView, SnapWay), daily.rs (today_book, commit), daily_commit.rs
//! (announce), day_pair.rs, notify/human.rs, deps.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::notify::human::{self as h, INFO, Msg, usd};
use crate::storage::journal::Level;

use super::day_pair::label;
use super::drawdown::{DrawView, Snap, SnapWay};
use super::engine::Core;

/// Журнал core.drawdown_snap; Telegram risk.daily_limit_reset — если запросил человек или снята пауза (lifted — какая).
/// Ошибок нет.
pub(super) async fn report(core: &Core, p: &str, s: &Snap, lifted: Option<&str>, human: bool) {
    let deals = |v: &[(String, Decimal)]| {
        v.iter()
            .map(|(id, a)| format!("{id} {}", usd(*a)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut text = format!(
        "Слепок баланса направления {} снят ({}; {}): на {} по факту {}",
        label(p),
        way_text(s.way),
        s.by,
        s.place,
        usd(s.fact_usd)
    );
    if !s.own.is_empty() {
        text.push_str(&format!(
            " + своё в пути {} ({})",
            usd(s.own_usd),
            deals(&s.own)
        ));
    }
    if !s.foreign.is_empty() {
        text.push_str(&format!(
            " − чужое на счёте {} ({})",
            usd(s.foreign_usd),
            deals(&s.foreign)
        ));
    }
    text.push_str(&format!(
        " = {}. Пик = слепок (решение 414)",
        usd(s.base_usd)
    ));
    if let Some(k) = lifted {
        text.push_str(&format!("; пауза {k} снята — направление снова торгует"));
    }
    let v = super::day_pair::venue_or_dex(core, p);
    let deps = &core.inner.deps;
    deps.log(Level::Info, "core.drawdown_snap", v.as_deref(), &text, None)
        .await;
    if human || lifted.is_some() {
        let tg = Msg::new(INFO, format!("Новый слепок: {}", label(p)))
            .line(format!("Слепок {} — он же пик", usd(s.base_usd)))
            .line(way_text(s.way))
            .line(
                lifted
                    .map(|k| format!("Пауза {k} снята — направление торгует"))
                    .unwrap_or_default(),
            )
            .render();
        deps.notify("risk.daily_limit_reset", v.as_deref(), &tg)
            .await;
    }
}

/// Слепок не снят: причина в запросе (экран) и журнал (только при смене причины). Ошибок нет.
pub(super) async fn note(core: &Core, p: &str, why: &str) {
    let (today, mut book) = super::daily::today_book(core);
    let Some(w) = book.draw_want.get_mut(p) else {
        return;
    };
    if w.note.as_deref() == Some(why) {
        return;
    }
    w.note = Some(why.to_string());
    let trip = super::daily::commit(core, today, book).await;
    super::daily_commit::announce(core, &trip).await;
    let text = format!(
        "Слепок баланса направления {} не снят: {why} — действует прежний пик; повтор через 2 минуты (решение 414)",
        label(p)
    );
    let v = super::day_pair::venue_or_dex(core, p);
    core.inner
        .deps
        .log(Level::Warn, "core.drawdown_snap", v.as_deref(), text, None)
        .await;
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
