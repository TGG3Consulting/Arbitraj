//! Файл: core/drawdown_ledger.rs
//! Что делает: решение 414 — своё и чужое по учёту сделок для слепка направления (плановые затраты возврата и «не
//! вернулось» — drawdown_pending.rs). Смотрит только сделки набора unsettled.rs (не закрытые, кроме «Беру на себя», или с
//! незаконченным возвратом USDT / выручкой, ждущей общего возврата группы докупки) — те же, что резерв возвратов
//! (usdt_hold.rs) и монеты других сделок на счёте (remain_others.rs). Пара сделки — по направлению (day_pair.rs).
//!
//! СВОЁ (сделки этой пары) — деньги, ушедшие с площадки покупки и ещё не вернувшиеся на неё:
//! - закрыта / «Беру на себя» с незаконченным возвратом: выручка, ждущая общего возврата группы (Deal.group.wait) +
//!   незаконченные части возврата («идёт», «ждёт повтора», «не вернулись» — без принятых площадкой выводов; решения
//!   138, 227; drawdown_pending.rs unreturned) — её итог уже в итоге дня;
//! - остановлена (Halted): вложено + её оценка в итоге дня (решения 53, 152) − выручка продаж на самой площадке
//!   покупки (она уже на счёте);
//! - идёт (покупка, перевод, продажа, застрявшая, ждёт человека): вложено − выручка продаж на площадке покупки (итог
//!   ещё не известен — по вложенному; когда станет известен, войдёт в итог пары).
//!
//! «Беру на себя» без незаконченного возврата в своё не входит (ревью t199, мелкое 2): её нет в наборе unsettled —
//! монеты у человека, на счёт бота сами не вернутся; «Вернуть боту» переносит долю оценки в новую сделку
//! (daily_load.rs transfer), и счёт старой «вложено + оценка» рядом с новой дал бы вложенное дважды. Её оценка и
//! поправки — в итоге пары после слепка, как у итога пары решения 258.
//!
//! ЧУЖОЕ (сделки других пар, чей дом — другой счёт) — их USDT на ЭТОМ счёте: у закрытых — строки резерва возвратов
//! usdt_hold.rs (уже без принятых площадкой выводов), у идущих и остановленных — выручка их продаж на этом счёте.
//! Сделки других пар с тем же счётом покупки (BingX → DEX и BingX → биржа делят счёт BingX) — общий котёл, не чужое.
//! Кто вызывает: drawdown_snap.rs (count, active), drawdown_pending.rs (pair_fn).
//! От чего зависит: unsettled.rs, usdt_hold.rs, usdt_return_parts.rs, day_pair.rs (of_kinds), deal.rs,
//! drawdown_fact.rs (Place), drawdown_pending.rs.
//! Настройки: нет.

use std::collections::{BTreeMap, HashSet};

use rust_decimal::Decimal;

use crate::gateway::contract::VenueKind;

use super::deal::{Deal, Step};
use super::drawdown_fact::Place;
use super::engine::{Core, lk, rd};

/// Своё и чужое направления по учёту.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Count {
    /// Идущие сделки пары (ID) — пусто: слепок «чистый».
    pub active: Vec<String>,
    pub own_usd: Decimal,
    pub own: Vec<(String, Decimal)>,
    pub foreign_usd: Decimal,
    pub foreign: Vec<(String, Decimal)>,
    /// Плановые затраты возврата USDT сделок пары.
    pub pending_cost: Decimal,
}

/// Выручка продаж сделки на счёте `pl` (продано × цена − комиссия в котируемой монете). Ошибок нет.
pub fn on_place(d: &Deal, pl: &Place, quote: &str) -> Decimal {
    d.sells
        .iter()
        .filter(|s| pl.same(&s.request.venue, d.dex_account.as_ref()))
        .map(|s| s.filled_qty * s.avg_price - s.fee_in(quote))
        .sum()
}

/// Что делает: своё у сделки пары (см. шапку). Принимает: сделку, площадку покупки, котируемую монету. Ошибок нет.
pub fn own_of(d: &Deal, pl: &Place, quote: &str) -> Decimal {
    match d.step {
        Step::Closed | Step::TakenOver => super::drawdown_pending::unreturned(d),
        Step::Halted => d.invested + d.provisional_usd.unwrap_or_default() - on_place(d, pl, quote),
        _ => d.invested - on_place(d, pl, quote),
    }
}

/// Пара сделки по видам площадок (снимок видов — без замка видов под замком сделок). Ошибок нет.
pub(super) fn pair_fn(core: &Core) -> impl Fn(&Deal) -> String {
    let kinds = rd(&core.inner.kinds).clone();
    move |d: &Deal| {
        super::day_pair::of_kinds(&d.direction, |v| {
            kinds
                .iter()
                .any(|(id, k)| id.0 == v && *k == VenueKind::Dex)
        })
    }
}

/// Что делает: своё и чужое пары `p` на её площадке покупки (см. шапку). Принимает: ядро, id пары, площадку
/// покупки, котируемую монету площадки. Возвращает: Count. Ошибок нет. Замки: deals, затем отдельно — резерв
/// возвратов (usdt_hold.rs берёт свои), не вложенно.
pub fn count(core: &Core, p: &str, pl: &Place, quote: &str) -> Count {
    let pair = pair_fn(core);
    let ids = super::unsettled::ids(core);
    let mut c = Count::default();
    let mut others: HashSet<String> = HashSet::new();
    let mut foreign: BTreeMap<String, Decimal> = BTreeMap::new();
    {
        let deals = lk(&core.inner.deals);
        for d in ids.iter().filter_map(|id| deals.get(id)) {
            if !super::unsettled::keep(d) {
                continue;
            }
            if pair(d) == p {
                c.active.push(d.id.clone());
                let o = own_of(d, pl, quote);
                if !o.is_zero() {
                    c.own.push((d.id.clone(), o));
                    c.own_usd += o;
                }
                c.pending_cost += super::drawdown_pending::pending_cost(d);
            } else if !pl.same(&d.direction.buy_on, d.dex_account.as_ref()) {
                others.insert(d.id.clone());
                if !matches!(d.step, Step::Closed | Step::TakenOver) {
                    let f = on_place(d, pl, quote);
                    if f > Decimal::ZERO {
                        *foreign.entry(d.id.clone()).or_default() += f;
                    }
                }
            }
        }
    }
    let h = super::usdt_hold::held(core, &pl.venue, pl.acc.as_ref());
    for (id, _, a) in h.lines.into_iter().filter(|(id, _, _)| others.contains(id)) {
        *foreign.entry(id).or_default() += a;
    }
    c.foreign_usd = foreign.values().copied().sum();
    c.foreign = foreign.into_iter().collect();
    c.active.sort();
    c.own.sort();
    c
}

/// Что делает: идущие сделки пары (набор unsettled, keep) — пусто: можно снимать «чистый» слепок. Принимает: ядро, id
/// пары. Возвращает: ID сделок. Ошибок нет. Замки: deals (коротко).
pub fn active(core: &Core, p: &str) -> Vec<String> {
    let pair = pair_fn(core);
    let ids = super::unsettled::ids(core);
    let deals = lk(&core.inner.deals);
    ids.iter()
        .filter_map(|id| deals.get(id))
        .filter(|d| super::unsettled::keep(d) && pair(d) == p)
        .map(|d| d.id.clone())
        .collect()
}
