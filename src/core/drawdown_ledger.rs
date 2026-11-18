//! Файл: core/drawdown_ledger.rs
//! Что делает: решение 414 — своё и чужое по учёту сделок для слепка направления и плановые затраты возврата USDT для
//! теоретического баланса. Смотрит только сделки набора unsettled.rs (не закрытые, кроме «Беру на себя», или с
//! незаконченным возвратом USDT / выручкой, ждущей общего возврата группы докупки) — те же, что резерв возвратов
//! (usdt_hold.rs) и монеты других сделок на счёте (remain_others.rs). Пара сделки — по направлению (day_pair.rs).
//!
//! СВОЁ (сделки этой пары) — деньги, ушедшие с площадки покупки и ещё не вернувшиеся на неё:
//! - закрыта / «Беру на себя» с незаконченным возвратом: выручка, ждущая общего возврата группы (Deal.group.wait) +
//!   незаконченные части возврата («идёт», «ждёт повтора», «не вернулись»; решения 138, 227) — её итог уже в итоге дня;
//! - остановлена (Halted): вложено + её оценка в итоге дня (решения 53, 152) − выручка продаж на самой площадке
//!   покупки (она уже на счёте);
//! - идёт (покупка, перевод, продажа, застрявшая, ждёт человека): вложено − выручка продаж на площадке покупки (итог
//!   ещё не известен — по вложенному; когда станет известен, войдёт в итог пары).
//!
//! ЧУЖОЕ (сделки других пар, чей дом — другой счёт) — их USDT на ЭТОМ счёте: у закрытых — строки резерва возвратов
//! usdt_hold.rs (уже без принятых площадкой выводов), у идущих и остановленных — выручка их продаж на этом счёте.
//! Сделки других пар с тем же счётом покупки (BingX → DEX и BingX → биржа делят счёт BingX) — общий котёл, не чужое.
//!
//! Плановые затраты возврата: у закрытой сделки пары с незаконченным возвратом — затраты стадии «возврат USDT» плана
//! (costs_types.rs RETURN_STAGE, в USDT) на долю незаконченного; после «вернулись» факт уже в итоге (решение 236).
//! Кто вызывает: drawdown_snap.rs (count), drawdown_limit.rs (pending).
//! От чего зависит: unsettled.rs, usdt_hold.rs, usdt_return_parts.rs, day_pair.rs (of_kinds), deal.rs,
//! drawdown_fact.rs (Place), uncapped_replay_plan.rs (PlanLine).
//! Настройки: нет.

use std::collections::{BTreeMap, HashSet};

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::gateway::contract::VenueKind;

use super::deal::{Deal, Step};
use super::drawdown_fact::Place;
use super::engine::{Core, lk, rd};
use super::usdt_return_state::UsdtReturn;

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

/// Сумма части возврата. Ошибок нет.
fn amount(p: &UsdtReturn) -> Decimal {
    match p {
        UsdtReturn::Pending { amount, .. }
        | UsdtReturn::Done { amount, .. }
        | UsdtReturn::NotReturned { amount, .. }
        | UsdtReturn::Retrying { amount, .. }
        | UsdtReturn::Manual { amount, .. } => *amount,
    }
}

/// Выручка, ждущая общего возврата группы. Ошибок нет.
fn wait(d: &Deal) -> Decimal {
    d.group
        .as_ref()
        .and_then(|g| g.wait.as_ref())
        .map_or(Decimal::ZERO, |(_, a)| *a)
}

/// Не вернулось: ждущая общего возврата выручка + незаконченные части. Ошибок нет.
pub fn unreturned(d: &Deal) -> Decimal {
    wait(d)
        + super::usdt_return_parts::all(d)
            .into_iter()
            .filter(|p| !p.finished())
            .map(amount)
            .sum::<Decimal>()
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
        Step::Closed | Step::TakenOver => unreturned(d),
        Step::Halted => d.invested + d.provisional_usd.unwrap_or_default() - on_place(d, pl, quote),
        _ => d.invested - on_place(d, pl, quote),
    }
}

/// Строки затрат плана (только нужное — costs_types.rs CostLine в JSON).
#[derive(Deserialize)]
struct PlanReturn {
    #[serde(default)]
    costs: Vec<super::uncapped_replay_plan::PlanLine>,
}

/// Что делает: плановые затраты возврата USDT закрытой сделки на долю незаконченного возврата. План не читается —
/// 0. Принимает: сделку. Ошибок нет.
pub fn pending_cost(d: &Deal) -> Decimal {
    if !matches!(d.step, Step::Closed | Step::TakenOver) {
        return Decimal::ZERO;
    }
    let left = unreturned(d);
    if left <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    let total = wait(d)
        + super::usdt_return_parts::all(d)
            .into_iter()
            .map(amount)
            .sum::<Decimal>();
    let Ok(p) = PlanReturn::deserialize(&d.plan) else {
        return Decimal::ZERO;
    };
    let plan: Decimal = p
        .costs
        .iter()
        .filter(|l| l.stage == super::costs_types::RETURN_STAGE)
        .filter_map(|l| l.usdt)
        .sum();
    if total <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    plan * (left / total).min(Decimal::ONE)
}

/// Пара сделки по видам площадок (снимок видов — без замка видов под замком сделок). Ошибок нет.
fn pair_fn(core: &Core) -> impl Fn(&Deal) -> String {
    let kinds = rd(&core.inner.kinds).clone();
    move |d: &Deal| {
        super::day_pair::of_kinds(&d.direction, |v| {
            kinds
                .iter()
                .any(|(id, k)| id.0 == v && *k == VenueKind::Dex)
        })
    }
}

/// Что делает: плановые затраты возврата по всем парам (один проход по набору unsettled). Принимает: ядро.
/// Возвращает: id пары → сумма (только ненулевые). Ошибок нет. Замки: deals (коротко).
pub fn pending(core: &Core) -> BTreeMap<String, Decimal> {
    let pair = pair_fn(core);
    let ids = super::unsettled::ids(core);
    let deals = lk(&core.inner.deals);
    let mut out: BTreeMap<String, Decimal> = BTreeMap::new();
    for d in ids.iter().filter_map(|id| deals.get(id)) {
        let c = pending_cost(d);
        if !c.is_zero() {
            *out.entry(pair(d)).or_default() += c;
        }
    }
    out
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
                c.pending_cost += pending_cost(d);
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
