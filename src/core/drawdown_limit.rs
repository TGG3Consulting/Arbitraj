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
//!
//! Повторное ревью: стоит пауза старого лимита дня, а просадка превышена — отметка «по просадке» ставится поверх.
//! Тексты (причина, Telegram) — drawdown_note.rs.
//!
//! Кто вызывает: daily_commit.rs (advance, check), risk.rs (snapshot — view_of, persist), daily_load.rs (restore),
//! daily_venue_limit.rs и daily_limit.rs (lift_legacy), drawdown_ask.rs (save).
//! От чего зависит: drawdown.rs, drawdown_cfg.rs, drawdown_note.rs (describe), daily.rs (Book), day_view.rs, day_pair.rs.
//! Настройки: risk.drawdown_pct, risk.drawdown_max_usd (через drawdown_cfg.rs).

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;

use super::daily::Book;
use super::day_pair::label;
use super::day_view::{DayView, PairDay};
use super::drawdown::DrawView;
use super::drawdown_note::describe;
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
            } else if v.exceeded && was && !r.draw_paused.contains(&p) {
                // Повторное ревью: стоит пауза старого лимита дня, а просадка превышена — пауза по просадке поверх
                // неё (иначе «Сбросить лимит» / полночь сняли бы старую и направление торговало бы с превышением).
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
        let by_draw = r.draw_paused.contains(&p);
        if let Some(e) = r.day.pairs.get_mut(&p) {
            e.draw_paused = by_draw;
            e.draw_on = true;
            e.limit = None;
            e.draw = v;
            e.paused = now;
        }
    }
    (paused, cleared)
}

/// Что делает: при запуске — паузы направлений по просадке и замеченная просадка из книги дня (ревью t199, мелкое 6:
/// перезапуск не даёт ложной паузы и повторной паузы после «Снять паузу»). Принимает: ядро, книгу. Ошибок нет.
pub(super) fn restore(core: &Core, book: &Book) {
    let mut r = lk(&core.inner.risk);
    for (p, why) in &book.draw_paused {
        r.draw_paused.insert(p.clone());
        r.pair_paused
            .entry(p.clone())
            .or_insert_with(|| why.clone());
    }
    for (p, v) in &book.draw_seen {
        r.draw_seen.entry(p.clone()).or_insert(*v);
    }
}

/// Что делает: ревью t199, С3 — направление перешло на просадку, а стоит пауза старого лимита дня (без отметки
/// draw_paused): «Сбросить лимит» снимает её сразу. Принимает: ядро, id пары. Возвращает: была ли снята. Ошибок нет.
pub(super) fn lift_legacy(core: &Core, p: &str) -> bool {
    let mut r = lk(&core.inner.risk);
    if r.draw_paused.contains(p) {
        return false;
    }
    r.loss_seen.remove(p);
    r.pair_paused.remove(p).is_some()
}

/// Что делает: записать паузы по просадке в учёт (после «Снять паузу» — risk.rs): пересчёт итога дня отдельной задачей.
/// Принимает: ядро. Ошибок нет.
pub fn persist(core: &Core) {
    let core = core.clone();
    tokio::spawn(async move { save(&core).await });
}

/// Что делает: пересчёт итога дня с записью в учёт (паузы по просадке, замеченная просадка) и объявлением того, что
/// сработало. Принимает: ядро. Ошибок нет.
pub async fn save(core: &Core) {
    let (today, book) = super::daily::today_book(core);
    let trip = super::daily::commit(core, today, book).await;
    super::daily_commit::announce(core, &trip).await;
}
