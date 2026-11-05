//! Файл: core/daily_venue.rs
//! Что делает: итог дня по площадкам (решение 173, Тигран 05.11.2026: «минус отдельно для MEXC, отдельно для BingX»).
//! Сделка относится к своим площадкам: CEX ↔ DEX — к бирже (CEX) сделки; CEX ↔ CEX — к обеим биржам (каждой —
//! весь итог сделки); DEX ↔ DEX — к DEX. Факт закрытой сделки и оценка позиции (решения 53, 152) идут каждой её
//! площадке; общий итог дня — по-прежнему сумма сделок без двойного счёта (daily.rs). Площадка без записи в реестре
//! шлюза (вид неизвестен) считается биржей — у неё свой учёт, а не общий с DEX. Убыток DEX копят только циклы
//! DEX ↔ DEX — поэтому его лимит останавливает только циклы (Risk.cycle_paused), а не направления CEX ↔ DEX.
//! Лимит и пауза площадки — daily_venue_limit.rs.
//! Кто вызывает: daily.rs (record, settle, today_book), daily_commit.rs (days), daily_load.rs, daily_migrate.rs,
//! daily_venue_limit.rs, daily_limit.rs (общий сброс), risk.rs (can_open — у каждой площадки сделки свой лимит).
//! От чего зависит: daily.rs (Book), day_view.rs (VenueDay), deal.rs, gateway/contract.rs (VenueKind), inner.kinds.
//! Настройки: нет.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::core::types::{Direction, VenueId};
use crate::gateway::contract::VenueKind;

use super::daily::Book;
use super::day_view::{DayView, LimitReset, VenueDay};
use super::deal::Deal;
use super::engine::{Core, rd};

/// Что делает: площадки направления для учёта дня: биржи сделки; если бирж нет (DEX ↔ DEX) — DEX.
/// Принимает: ядро (виды площадок реестра), направление. Возвращает: id площадок без повторов. Ошибок нет.
pub fn of_direction(core: &Core, d: &Direction) -> Vec<String> {
    let kinds = rd(&core.inner.kinds);
    let dex = |v: &VenueId| kinds.iter().any(|(id, k)| id == v && *k == VenueKind::Dex);
    let mut all = vec![d.buy_on.clone()];
    if d.sell_on != d.buy_on {
        all.push(d.sell_on.clone());
    }
    let cex: Vec<String> = all
        .iter()
        .filter(|v| !dex(v))
        .map(|v| v.0.clone())
        .collect();
    if cex.is_empty() {
        all.into_iter().map(|v| v.0).collect()
    } else {
        cex
    }
}

/// Площадка — DEX (по виду в реестре шлюза). Принимает: ядро, id. Ошибок нет.
pub fn is_dex(core: &Core, v: &str) -> bool {
    rd(&core.inner.kinds)
        .iter()
        .any(|(id, k)| id.0 == v && *k == VenueKind::Dex)
}

/// Чей это учёт дня для человека: у биржи — её имя («MEXC»), у DEX — «DEX ↔ DEX» (убыток DEX копят только циклы
/// DEX ↔ DEX — сделки CEX ↔ DEX идут бирже). Принимает: ядро, id. Ошибок нет.
pub fn label(core: &Core, v: &str) -> String {
    let n = crate::notify::human::venue(v);
    if is_dex(core, v) {
        format!("{n} ↔ {n}")
    } else {
        n
    }
}

/// Площадки сделки: запомненные у её оценки (книга дня), иначе — по направлению. Ошибок нет.
pub(super) fn of_deal(core: &Core, book: &Book, deal: &Deal) -> Vec<String> {
    book.deal_venues
        .get(&deal.id)
        .cloned()
        .unwrap_or_else(|| of_direction(core, &deal.direction))
}

/// Добавляет сумму к факту дня каждой площадки сделки. Принимает: книгу, площадки, сумму. Ошибок нет.
pub(super) fn add_final(book: &mut Book, venues: &[String], delta: Decimal) {
    for v in venues {
        *book.venue_final.entry(v.clone()).or_default() += delta;
    }
}

/// Факт дня по площадкам из итога дня в памяти (для книги дня). Ошибок нет.
pub(super) fn finals(day: &DayView) -> BTreeMap<String, Decimal> {
    day.venues
        .iter()
        .filter(|(_, x)| !x.final_usd.is_zero())
        .map(|(v, x)| (v.clone(), x.final_usd))
        .collect()
}

/// Отметки сброса лимита площадок из итога дня в памяти (для книги дня). Ошибок нет.
pub(super) fn resets(day: &DayView) -> BTreeMap<String, LimitReset> {
    day.venues
        .iter()
        .filter_map(|(v, x)| x.limit_reset.clone().map(|r| (v.clone(), r)))
        .collect()
}

/// Что делает: итог дня по площадкам из книги (без лимитов — их ставит daily_venue_limit.rs). Площадки: все из
/// реестра шлюза и все, у кого есть факт, оценка этого дня или сброс.
/// Принимает: ядро, книгу, день. Возвращает: id площадки → итог. Ошибок нет.
pub(super) fn days(core: &Core, book: &Book, date: NaiveDate) -> BTreeMap<String, VenueDay> {
    let mut out: BTreeMap<String, VenueDay> = rd(&core.inner.kinds)
        .iter()
        .map(|(id, _)| (id.0.clone(), VenueDay::default()))
        .collect();
    for (v, f) in &book.venue_final {
        out.entry(v.clone()).or_default().final_usd += *f;
    }
    for (id, (amount, day)) in &book.provisional {
        if *day != date {
            continue;
        }
        for v in book.deal_venues.get(id).into_iter().flatten() {
            out.entry(v.clone()).or_default().provisional_usd += *amount;
        }
    }
    for (v, r) in &book.venue_resets {
        out.entry(v.clone()).or_default().limit_reset = Some(r.clone());
    }
    for x in out.values_mut() {
        x.result_usd = x.final_usd + x.provisional_usd;
    }
    out
}
