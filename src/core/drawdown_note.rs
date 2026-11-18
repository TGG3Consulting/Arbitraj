//! Файл: core/drawdown_note.rs
//! Что делает: решение 414 — тексты снятия слепка баланса направления (вынесено из drawdown_snap.rs): снят — журнал
//! core.drawdown_snap (факт, своё в пути, чужое на счёте с ID сделок, итог, какая пауза снята), Telegram
//! risk.daily_limit_reset, если запросил человек или снята пауза; не снят — причина в запросе (экран, Want.note) и
//! журнал при смене причины.
//! Кто вызывает: drawdown_snap.rs (apply, take).
//! От чего зависит: drawdown.rs (Snap), drawdown_limit.rs (way_text), daily.rs (today_book, commit), daily_commit.rs
//! (announce), day_pair.rs, notify/human.rs, deps.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::notify::human::{INFO, Msg, usd};
use crate::storage::journal::Level;

use super::day_pair::label;
use super::drawdown::Snap;
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
        super::drawdown_limit::way_text(s.way),
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
            .line(super::drawdown_limit::way_text(s.way))
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
