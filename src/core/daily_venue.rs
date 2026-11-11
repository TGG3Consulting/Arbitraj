//! Файл: core/daily_venue.rs
//! Что делает: итог дня по площадкам (решение 173, Тигран 05.11.2026: «минус отдельно для MEXC, отдельно для BingX»;
//! дополнение t91, Тигран 11.11.2026: «биржа ↔ биржа — бирже покупки, как в отчёте»).
//! Сделка относится к ОДНОЙ площадке (owner — единственное место правила; им же пользуется отчёт «Итоги сделок»,
//! report/rows_text.rs): CEX ↔ DEX — к бирже (CEX) сделки; CEX ↔ CEX — к бирже ПОКУПКИ; DEX ↔ DEX — к DEX. Факт закрытой
//! сделки и оценка позиции (решения 53, 152) идут этой площадке; сумма по площадкам = общий итог дня (daily.rs).
//! Площадка без записи в реестре шлюза (вид неизвестен) считается биржей — у неё свой учёт, а не общий с DEX. Убыток
//! DEX копят только циклы DEX ↔ DEX — поэтому его лимит останавливает только циклы (Risk.cycle_paused), а не
//! направления CEX ↔ DEX. Лимит и пауза площадки — daily_venue_limit.rs.
//! Вход в сделку (t91 не менял): какие площадки направления проверяются на паузу циклов и на заданный лимит дня —
//! entry_venues (прежнее правило: все биржи направления, бирж нет — DEX); пауза любой из двух площадок направления
//! (Risk.venue_paused) по-прежнему не даёт начать сделку (risk.rs can_open).
//! Запись итога дня по прежнему правилу (биржа ↔ биржа — обеим) приводится к новому при загрузке — daily_rule.rs.
//! Кто вызывает: daily.rs (record, settle, today_book), daily_commit.rs (days, RULE), daily_load.rs, daily_migrate.rs,
//! daily_rule.rs, daily_venue_limit.rs, daily_limit.rs (общий сброс), risk.rs и risk_ready.rs (entry_venues),
//! report/rows_text.rs (owner).
//! От чего зависит: daily.rs (Book), day_view.rs (VenueDay), deal.rs, gateway/contract.rs (VenueKind), inner.kinds.
//! Настройки: нет.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;

use crate::core::types::Direction;
use crate::gateway::contract::VenueKind;

use super::daily::Book;
use super::day_view::{DayView, LimitReset, VenueDay};
use super::deal::Deal;
use super::engine::{Core, rd};

/// Версия правила площадок, по которой записана разбивка книги дня (Book.venue_rule): 0 (поля нет) — решение 173
/// (биржа ↔ биржа — обеим биржам), 1 — t91 (бирже покупки). Пишет daily_commit.rs, читает daily_load.rs.
pub(super) const RULE: u8 = 1;

/// Что делает: ЕДИНСТВЕННОЕ правило «чья сделка» в итоге дня и в дневных лимитах (решение 173, дополнение t91):
/// покупка на бирже (CEX ↔ DEX, CEX ↔ CEX) — биржа покупки; покупка на DEX, продажа на бирже — биржа продажи;
/// обе стороны DEX (DEX ↔ DEX) — DEX (площадка покупки).
/// Принимает: id площадки покупки и продажи, «площадка — DEX». Возвращает: id одной площадки. Ошибок нет.
pub fn owner(buy: &str, sell: &str, dex: impl Fn(&str) -> bool) -> String {
    if !dex(buy) || dex(sell) {
        buy.to_string()
    } else {
        sell.to_string()
    }
}

/// Что делает: площадка учёта дня направления (owner по видам площадок реестра шлюза).
/// Принимает: ядро, направление. Возвращает: id одной площадки (вектор — формат Book.deal_venues). Ошибок нет.
pub fn of_direction(core: &Core, d: &Direction) -> Vec<String> {
    vec![owner(&d.buy_on.0, &d.sell_on.0, |v| is_dex(core, v))]
}

/// Что делает: площадки направления для ВХОДА в сделку (t91 вход не менял — прежнее правило решения 173): биржи
/// направления; если бирж нет (DEX ↔ DEX) — DEX. По ним risk.rs проверяет паузу циклов DEX ↔ DEX, risk_ready.rs —
/// заданный risk.max_daily_loss_usd. Принимает: ядро, направление. Возвращает: id площадок без повторов. Ошибок нет.
pub fn entry_venues(core: &Core, d: &Direction) -> Vec<String> {
    let mut all = vec![d.buy_on.0.clone()];
    if d.sell_on != d.buy_on {
        all.push(d.sell_on.0.clone());
    }
    let cex: Vec<String> = all.iter().filter(|v| !is_dex(core, v)).cloned().collect();
    if cex.is_empty() { all } else { cex }
}

/// Направление биржа ↔ биржа: обе площадки — не DEX и разные. Принимает: ядро, направление. Ошибок нет.
pub(super) fn is_cex_cex(core: &Core, d: &Direction) -> bool {
    d.buy_on != d.sell_on && !is_dex(core, &d.buy_on.0) && !is_dex(core, &d.sell_on.0)
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

/// Добавляет сумму к факту дня площадки сделки (вектор — формат Book.deal_venues). Принимает: книгу, площадки, сумму. Ошибок нет.
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
