//! Файл: core/drawdown_pending.rs
//! Что делает: решение 414 — плановые затраты возврата USDT в теоретическом балансе направления (вынесено из
//! drawdown_ledger.rs) и «не вернулось» у сделки. У закрытой сделки пары с незаконченным возвратом — затраты стадии
//! «возврат USDT» плана (costs_types.rs RETURN_STAGE, в USDT) на долю незаконченного; после «вернулись» факт уже в
//! итоге сделки (решение 236).
//! Ревью t199, С1: в ledger::finish итог дня (daily::settle → commit) идёт ДО решения о возврате и сохранения сделки —
//! в учёте ядра она ещё без частей возврата. Поэтому сделка, ради которой пересчитывается итог дня, передаёт свою
//! плановую стоимость возврата сама (Book.cur → Cur, ставят daily.rs settle и record): закрывается — то, что будет
//! после usdt_return_deal::prepare (возврат нужен, выручка не на площадке покупки — весь план; нет — 0); поправка
//! закрытой (копия с частями) — по её частям. Остальные сделки — по учёту ядра (набор unsettled.rs).
//! «Не вернулось» (ревью t199, мелкое 2): «не вернулись» с принятым площадкой выводом (usdt_accepted.rs) — деньги со
//! счёта-отправителя ушли (а дошли ли — после перезапуска не видно; по BSC перевод идёт секунды-минуты) — принятое
//! вычитается; у «идёт» — нет: деньги в пути и на счёте покупки их ещё нет, а зачисление сразу делает часть
//! «вернулись». Куски общего возврата группы: дошедший кусок уменьшает сумму части до остатка (usdt_return_pieces.rs)
//! — двойного счёта нет.
//! Кто вызывает: drawdown_ledger.rs (count, own_of), daily_commit.rs (pending), daily.rs (cur).
//! От чего зависит: usdt_return_parts.rs, usdt_accepted.rs (sent_of), usdt_return_deal.rs (wanted, multi),
//! uncapped_replay_plan.rs (PlanLine), day_pair.rs, unsettled.rs, drawdown_ledger.rs (pair_fn).
//! Настройки: нет (usdt_return.mode — через usdt_return_deal::wanted).

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::Deserialize;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};
use super::usdt_return_state::UsdtReturn;

/// Сделка, ради которой пересчитывается итог дня: её пара и плановые затраты возврата (С1).
#[derive(Debug, Clone, PartialEq)]
pub struct Cur {
    pub id: String,
    pub pair: String,
    pub cost: Decimal,
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

/// Что делает: не вернулось — ждущая общего возврата выручка + незаконченные части («не вернулись» — без принятых
/// площадкой выводов, см. шапку). Принимает: сделку. Ошибок нет.
pub fn unreturned(d: &Deal) -> Decimal {
    let mut sum = wait(d);
    for p in super::usdt_return_parts::all(d) {
        match p {
            _ if p.finished() => {}
            UsdtReturn::NotReturned { amount, at, .. } => {
                let (sent, _) = super::usdt_accepted::sent_of(d, at, *amount);
                sum += (*amount - sent).max(Decimal::ZERO);
            }
            _ => sum += amount(p),
        }
    }
    sum
}

/// Строки затрат плана (только нужное — costs_types.rs CostLine в JSON).
#[derive(Deserialize)]
struct PlanReturn {
    #[serde(default)]
    costs: Vec<super::uncapped_replay_plan::PlanLine>,
}

/// Затраты стадии «возврат USDT» плана сделки в USDT (план не читается — 0). Ошибок нет.
pub fn plan_return(d: &Deal) -> Decimal {
    let Ok(p) = PlanReturn::deserialize(&d.plan) else {
        return Decimal::ZERO;
    };
    p.costs
        .iter()
        .filter(|l| l.stage == super::costs_types::RETURN_STAGE)
        .filter_map(|l| l.usdt)
        .sum()
}

/// Что делает: плановые затраты возврата закрытой сделки на долю незаконченного возврата (по её частям).
/// Принимает: сделку. Ошибок нет.
pub fn pending_cost(d: &Deal) -> Decimal {
    if !matches!(d.step, Step::Closed | Step::TakenOver) {
        return Decimal::ZERO;
    }
    let left = unreturned(d);
    let total = wait(d)
        + super::usdt_return_parts::all(d)
            .into_iter()
            .map(amount)
            .sum::<Decimal>();
    if left <= Decimal::ZERO || total <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    plan_return(d) * (left / total).min(Decimal::ONE)
}

/// Что делает: С1 — плановые затраты возврата сделки, ради которой пересчитывается итог дня. Закрыта и частей
/// возврата ещё нет — как решит usdt_return_deal::prepare: возврат нужен (usdt_return.mode = per_trade или «Продать и
/// перегнать») и выручка не только на площадке покупки — весь план; иначе — по частям (pending_cost).
/// Принимает: ядро, сделку. Возвращает: Cur. Ошибок нет.
pub fn cur(core: &Core, d: &Deal) -> Cur {
    let fresh =
        d.step == Step::Closed && super::usdt_return_parts::all(d).is_empty() && wait(d).is_zero();
    let cost = if fresh {
        let away = d.at != d.direction.buy_on || super::usdt_return_parts::multi(d);
        if away && super::usdt_return_deal::wanted(core, d) && d.proceeds > Decimal::ZERO {
            plan_return(d)
        } else {
            Decimal::ZERO
        }
    } else {
        pending_cost(d)
    };
    Cur {
        id: d.id.clone(),
        pair: super::day_pair::of_direction(core, &d.direction),
        cost,
    }
}

/// Что делает: плановые затраты возврата по всем парам (один проход по набору unsettled); сделка `cur` — по своему
/// значению вместо записи учёта ядра. Принимает: ядро, текущую сделку пересчёта. Возвращает: id пары → сумма (только
/// ненулевые). Ошибок нет. Замки: deals (коротко).
pub fn pending(core: &Core, cur: Option<&Cur>) -> BTreeMap<String, Decimal> {
    let pair = super::drawdown_ledger::pair_fn(core);
    let ids = super::unsettled::ids(core);
    let mut out: BTreeMap<String, Decimal> = BTreeMap::new();
    {
        let deals = lk(&core.inner.deals);
        for d in ids.iter().filter_map(|id| deals.get(id)) {
            if cur.is_some_and(|c| c.id == d.id) {
                continue;
            }
            let c = pending_cost(d);
            if !c.is_zero() {
                *out.entry(pair(d)).or_default() += c;
            }
        }
    }
    if let Some(c) = cur.filter(|c| !c.cost.is_zero()) {
        *out.entry(c.pair.clone()).or_default() += c.cost;
    }
    out
}
