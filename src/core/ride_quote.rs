//! Файл: core/ride_quote.rs
//! Что делает: котировки новой сделки монеты, пока идут её прошлые сделки (задача t179, решение 390; coin_ride.rs) —
//! чтобы две сделки не считали одну и ту же ликвидность:
//! - ПРОДАЖА на B: монеты идущих сделок, которые будут проданы на той же площадке B (перевод ещё идёт или продажа
//!   идёт), заберут лучшую часть стакана / пула. Новая сделка считается так, будто они уже проданы: котировка на
//!   (их монеты + наш объём) минус котировка на их монеты — наша выручка — это то, что останется ПОСЛЕ них (как
//!   случай Я-2 / XPL 02.11: вторая сделка считала стакан / пул до первой). Затраты: пропорциональные (комиссия
//!   биржи, комиссия пула, налоги, влияние крючка) — тоже разностью; газ, approve, обёртка, чаевые — одного нашего
//!   обмена (как в котировке на полный объём).
//! - ПОКУПКА на A: идущая сделка купила на той же площадке A — данные должны быть новее её покупки: у биржи стакан
//!   получен позже конца её ордера, у DEX маршрут через её пулы посчитан по блоку не раньше блока её покупки. Иначе
//!   отказ «данные старше покупки сделки …» (расчёт повторит следующий тик).
//!
//! Только живой расчёт (снимок рынка решения 217 — без изменений). Идущих сделок нет — котировки как раньше.
//! Кто вызывает: costs_source.rs (Live: quote, fillable, worst).
//! От чего зависит: coin_ride.rs, costs_quote.rs, engine.rs, core/types.
//! Настройки: нет (предел сделок монеты — coin_ride.rs).

use std::collections::HashSet;

use rust_decimal::Decimal;

use crate::core::types::{Cost, CostKind, DexRoute, Direction, PoolRef, Quote};

use super::coin_key::CoinKey;
use super::engine::{Core, lk};
use super::size::Fail;

/// Что делает: сколько монет идущих сделок монеты будет продано на площадке продажи направления. Принимает: ядро,
/// направление. Возвращает: количество (0 — нет). Ошибок нет.
pub fn sell_ahead(core: &Core, d: &Direction) -> Decimal {
    let rides = super::coin_ride::of_coin(core, &CoinKey::of(&d.asset));
    if rides.is_empty() {
        return Decimal::ZERO;
    }
    let deals = lk(&core.inner.deals);
    rides
        .iter()
        .filter(|r| r.sell_on == d.sell_on)
        .filter_map(|r| deals.get(&r.deal_id))
        .filter(|x| x.is_open())
        .map(|x| x.qty)
        .sum()
}

/// Затрата одного обмена, не зависящая от объёма (газ, approve, обёртка, чаевые). Ошибок нет.
fn fixed(k: CostKind) -> bool {
    matches!(
        k,
        CostKind::Gas
            | CostKind::ApproveGas
            | CostKind::TransferGas
            | CostKind::WrapGas
            | CostKind::ChannelTip
    )
}

/// Что делает: наша часть продажи после монет идущих сделок. Принимает: котировку на (ahead + qty), котировку на
/// ahead, наш объём, ahead. Возвращает: Quote на qty: исполнимо = исполнимо полной − ahead (не меньше 0, не больше
/// qty), средняя = (выручка полной − выручка ahead) / исполнимо; затраты — см. шапку; маршрут и время — полной.
/// Ошибок нет (исполнимо 0 — средняя 0, глубину отсечёт costs.rs).
pub fn marginal(total: &Quote, ahead: &Quote, qty: Decimal, off: Decimal) -> Quote {
    let fill = (total.fillable_qty - off).max(Decimal::ZERO).min(qty);
    let usd = total.avg_price * total.fillable_qty - ahead.avg_price * ahead.fillable_qty;
    // Глубины хватает на ahead (исполнимо ahead = off) — наши монеты ровно fill, их выручка — usd. Не хватает даже на
    // ahead — у полной исполнимо столько же (вся глубина), fill = 0.
    let avg = if fill.is_zero() {
        Decimal::ZERO
    } else {
        (usd / fill).max(Decimal::ZERO)
    };
    let costs = total
        .costs
        .iter()
        .map(|c| {
            if fixed(c.kind) {
                return c.clone();
            }
            let before: Decimal = ahead
                .costs
                .iter()
                .filter(|x| {
                    x.kind == c.kind
                        && x.asset == c.asset
                        && x.in_price == c.in_price
                        && x.step == c.step
                })
                .map(|x| x.amount)
                .sum();
            Cost {
                amount: (c.amount - before).max(Decimal::ZERO),
                ..c.clone()
            }
        })
        .collect();
    Quote {
        qty,
        fillable_qty: fill,
        avg_price: avg,
        costs,
        ..total.clone()
    }
}

/// Что делает: живая котировка продажи на B после монет идущих сделок (ahead > 0). Принимает: ядро, направление, наш
/// объём, ahead. Возвращает: marginal. Ошибки: как у costs_quote::fetch (на любой из двух котировок).
pub async fn sell_after(
    core: &Core,
    d: &Direction,
    qty: Decimal,
    off: Decimal,
) -> Result<Quote, Fail> {
    let total = Box::pin(super::costs_quote::fetch(core, d, true, qty + off, None)).await?;
    let ahead = Box::pin(super::costs_quote::fetch(core, d, true, off, None)).await?;
    Ok(marginal(&total, &ahead, qty, off))
}

/// Пулы маршрута. Ошибок нет.
fn pools(r: &DexRoute) -> HashSet<&PoolRef> {
    r.legs
        .iter()
        .flat_map(|l| l.hops.iter().map(|h| &h.pool))
        .collect()
}

/// Что делает: данные покупки новее покупок идущих сделок на той же площадке A (см. шапку). Принимает: ядро,
/// направление, живую котировку покупки. Ошибки: текст причины (данные старше покупки идущей сделки; блок её покупки
/// неизвестен).
pub fn buy_fresh(core: &Core, d: &Direction, q: &Quote) -> Result<(), String> {
    let rides = super::coin_ride::of_coin(core, &CoinKey::of(&d.asset));
    if rides.is_empty() {
        return Ok(());
    }
    let deals = lk(&core.inner.deals);
    for r in rides.iter().filter(|r| r.buy_on == d.buy_on) {
        let Some(b) = deals.get(&r.deal_id).and_then(|x| x.buy.as_ref()) else {
            continue;
        };
        match (&q.route, &b.request.route) {
            (None, _) => {
                if q.at <= b.finished_at {
                    return Err(format!(
                        "{}: стакан получен {} — не позже конца покупки сделки {} ({}), ждём кадр после неё",
                        d.buy_on.0, q.at, r.deal_id, b.finished_at
                    ));
                }
            }
            (Some(mine), Some(theirs)) => {
                let theirs = pools(theirs);
                if !pools(mine).iter().any(|p| theirs.contains(p)) {
                    continue;
                }
                match b.chain.as_ref().and_then(|c| c.block) {
                    Some(bl) if mine.block >= bl => {}
                    Some(bl) => {
                        return Err(format!(
                            "{}: маршрут по блоку {} — раньше блока {bl} покупки сделки {} в тех же пулах",
                            d.buy_on.0, mine.block, r.deal_id
                        ));
                    }
                    None => {
                        return Err(format!(
                            "{}: блок покупки сделки {} в тех же пулах неизвестен — нечем проверить свежесть пулов",
                            d.buy_on.0, r.deal_id
                        ));
                    }
                }
            }
            (Some(_), None) => {}
        }
    }
    Ok(())
}
