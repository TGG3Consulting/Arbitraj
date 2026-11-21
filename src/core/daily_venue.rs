//! Файл: core/daily_venue.rs
//! Что делает: итог дня по парам «площадка × тип направления» (решение 258, Тигран 12.11.2026: «Каждое направление
//! имеет свой лимит. Все.»; было — по площадке, решение 173 с дополнением t91). Сделка относится к ОДНОЙ паре
//! (core/day_pair.rs — единственное правило; им же пользуется отчёт «Итоги сделок», report/rows_text.rs): биржа → DEX —
//! биржа · «биржа → DEX»; DEX → биржа — биржа · «DEX → биржа»; биржа → биржа — биржа ПОКУПКИ · «биржа → биржа»;
//! DEX ↔ DEX — общая пара «DEX ↔ DEX». Факт закрытой сделки и оценка позиции (решения 53, 152) идут этой паре; сумма по
//! парам = общий итог дня (daily.rs). Лимит и пауза пары — daily_venue_limit.rs.
//! Запись итога дня по площадкам (до решения 258) разносится по парам при загрузке — daily_migrate.rs.
//! Кто вызывает: daily.rs (record, settle, today_book), daily_commit.rs (days), daily_load.rs, daily_migrate.rs,
//! daily_venue_limit.rs, daily_limit.rs (общий сброс), risk.rs (snapshot), report (is_dex).
//! От чего зависит: daily.rs (Book), day_pair.rs, day_view.rs (PairDay), deal.rs, gateway/contract.rs (VenueKind),
//! inner.kinds.
//! Настройки: нет.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::gateway::contract::VenueKind;

use super::daily::Book;
use super::day_view::{DayView, LimitReset, PairDay};
use super::deal::Deal;
use super::engine::{Core, rd};

/// Площадка — DEX (по виду в реестре шлюза). Принимает: ядро, id. Ошибок нет.
pub fn is_dex(core: &Core, v: &str) -> bool {
    rd(&core.inner.kinds)
        .iter()
        .any(|(id, k)| id.0 == v && *k == VenueKind::Dex)
}

/// Пара сделки: запомненная у её оценки (книга дня), иначе — по направлению (day_pair.rs). Ошибок нет.
pub(super) fn of_deal(core: &Core, book: &Book, deal: &Deal) -> String {
    book.deal_pairs
        .get(&deal.id)
        .cloned()
        .unwrap_or_else(|| super::day_pair::of_direction(core, &deal.direction))
}

/// Добавляет сумму к факту дня пары. Принимает: книгу, id пары, сумму. Ошибок нет.
pub(super) fn add_final(book: &mut Book, pair: &str, delta: Decimal) {
    *book.pair_final.entry(pair.to_string()).or_default() += delta;
}

/// Факт дня по парам из итога дня в памяти (для книги дня). Ошибок нет.
pub(super) fn finals(day: &DayView) -> BTreeMap<String, Decimal> {
    day.pairs
        .iter()
        .filter(|(_, x)| !x.final_usd.is_zero())
        .map(|(p, x)| (p.clone(), x.final_usd))
        .collect()
}

/// Отметки сброса лимита пар из итога дня в памяти (для книги дня). Ошибок нет.
pub(super) fn resets(day: &DayView) -> BTreeMap<String, LimitReset> {
    day.pairs
        .iter()
        .filter_map(|(p, x)| x.limit_reset.clone().map(|r| (p.clone(), r)))
        .collect()
}

/// Пустой итог пары с подписями (биржа, тип, подпись для человека). Ошибок нет.
fn empty(p: &str) -> PairDay {
    PairDay {
        venue: super::day_pair::venue_of(p).map(str::to_string),
        kind: super::day_pair::kind_of(p)
            .map(|k| k.key().to_string())
            .unwrap_or_default(),
        label: super::day_pair::label(p),
        ..PairDay::default()
    }
}

/// Что делает: итог дня по парам из книги (без лимитов — их ставит daily_venue_limit.rs). Пары: все пары реестра шлюза
/// (day_pair::all) и все, у кого есть факт, оценка этого дня или сброс.
/// Принимает: ядро, книгу, день. Возвращает: id пары → итог. Ошибок нет.
pub(super) fn days(core: &Core, book: &Book, date: NaiveDate) -> BTreeMap<String, PairDay> {
    let mut out: BTreeMap<String, PairDay> = super::day_pair::all(core)
        .into_iter()
        .map(|p| {
            let x = empty(&p);
            (p, x)
        })
        .collect();
    for (p, f) in &book.pair_final {
        out.entry(p.clone()).or_insert_with(|| empty(p)).final_usd += *f;
    }
    for (id, (amount, day)) in &book.provisional {
        if *day != date {
            continue;
        }
        if let Some(p) = book.deal_pairs.get(id) {
            out.entry(p.clone())
                .or_insert_with(|| empty(p))
                .provisional_usd += *amount;
        }
    }
    for (p, r) in &book.pair_resets {
        out.entry(p.clone()).or_insert_with(|| empty(p)).limit_reset = Some(r.clone());
    }
    for x in out.values_mut() {
        x.result_usd = x.final_usd + x.provisional_usd;
    }
    out
}
