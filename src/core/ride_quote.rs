//! Файл: core/ride_quote.rs
//! Что делает: котировки докупки (решение 394, t179; записка 269, п. 4: ликвидность, которую заберут идущие сделки
//! группы, вычитается с ОБЕИХ сторон) — пока идут сделки группы докупки той же монеты (coin_ride.rs):
//! - ПРОДАЖА на B: монеты идущих сделок группы будут проданы на той же B и заберут лучшую часть стакана / пула. Докупка
//!   считается так, будто они уже проданы: котировка на (их монеты + наш объём) минус котировка на их монеты — наша
//!   выручка после них (случай Я-2 / XPL 02.11: вторая сделка считала стакан / пул до первой). Затраты:
//!   пропорциональные (комиссия биржи, комиссия пула, налоги, влияние крючка) — разностью; газ, approve, обёртка,
//!   чаевые — одного нашего обмена.
//! - ПОКУПКА на A: идущие сделки группы уже купили — их ликвидность снята с A самой площадкой; докупка считается только
//!   по данным ПОСЛЕ их покупки (объём сверх купленного): у биржи — стакан получен позже конца её ордера; у DEX — маршрут
//!   через её пулы (пулы одного контракта — один пул, PoolRef::site) по блоку не раньше её блока. Котировка без маршрута
//!   (агрегатор) или покупка без маршрута / блока у DEX — «не доказуемо свежее» — отказ. Расчёт повторит следующий тик.
//!
//! Решение 405 (t191): идущие сделки ВСЕХ групп монеты (и других направлений) — продажа учитывает те, что продают на
//! той же площадке продажи, покупка — те, что купили на той же площадке покупки (фильтры sell_ahead / buy_fresh).
//! Только живой расчёт сделки (снимок рынка решения 217 и теневой замер решения 267 — без изменений, costs_source.rs)
//! и только сделки с группой докупки (exec.top_up = да): выключено — ничего не меняется.
//! Решение 464 (t238, Тигран 20.11): котировка продажи = пул как есть + сдвиг от ВСЕХ своих идущих сделок монеты,
//! которые ещё продадут на той же площадке (любой биржи покупки, любой группы — фильтр только sell_on; монеты, уже
//! проданные в блок котировки, — в состоянии пула и в остаток не входят: Deal.qty уменьшен, след ride_trail); наша
//! продажа в полёте или исполнившаяся за время котировки — отказ до её итога (ride_settled.rs).
//! Решение 480 (Тигран 21.11): к сдвигу добавляется остаток наших продаж с неизвестным исходом на той же площадке
//! (ride_settled::worst — как ещё не исполненных, худший случай; и без групп и при выключенной докупке). Ревью 2: сдвиг
//! снимается один раз на расчёт (ride_shift.rs) и передаётся live_sell; пара котировок marginal — по одному состоянию
//! площадки (ride_pair.rs).
//! Кто вызывает: costs_source.rs (Live: idle, live_buy, live_sell, sell_ahead через ride_shift), precheck_dex_sell.rs
//! (ahead), ride_shift.rs (idle, sell_ahead), partial_back.rs (marginal).
//! От чего зависит: coin_ride.rs, costs_quote.rs, ride_trail.rs (sites, check), ride_settled.rs (before / after),
//! ride_shift.rs, ride_pair.rs, trade_ops.rs, engine.rs, core/types.
//! Настройки: нет (включатель — coin_ride.rs).

use rust_decimal::Decimal;

use crate::core::types::{Cost, CostKind, Direction, Quote};
use crate::gateway::contract::VenueKind;

use super::coin_key::CoinKey;
use super::coin_ride::Ride;
use super::engine::{Core, lk};
use super::size::Fail;

/// Идущие сделки ГРУПП докупки монеты (у сделок без группы — пусто: выключенная докупка ничего не меняет). Ошибок нет.
fn group_rides(core: &Core, d: &Direction) -> Vec<Ride> {
    let key = CoinKey::of(&d.asset);
    let r = lk(&core.inner.rides);
    match r.get(&key) {
        Some(v) if v.iter().any(|x| x.group.is_some()) => {
            v.iter().filter(|x| x.group.is_some()).cloned().collect()
        }
        _ => vec![],
    }
}

/// Что делает: решение 394 — котировки и затраты живого расчёта как раньше (costs_source.rs, быстрый выход без новых
/// будущих): нет идущих сделок групп докупки и следов их продаж (счётчик inner.group_live — без замков), докупка
/// выключена или по монете направления групп нет. Принимает: ядро, направление. Ошибок нет.
pub fn idle(core: &Core, d: &Direction) -> bool {
    // Ревью 2 t179: групп и следов нет — без замков и ключа монеты; докупку выключили — идущие группы доводятся, но на
    // новые расчёты не действуют.
    if core
        .inner
        .group_live
        .load(std::sync::atomic::Ordering::Relaxed)
        == 0
        || !super::coin_ride::top_up_on(core)
    {
        return true;
    }
    let key = CoinKey::of(&d.asset);
    let none = lk(&core.inner.rides)
        .get(&key)
        .is_none_or(|v| v.iter().all(|x| x.group.is_none()));
    none && lk(&core.inner.trail).is_empty()
}

/// Что делает: живая котировка покупки на A при идущих сделках групп (вызывает costs_source.rs Live, в куче) — по данным
/// новее их покупки (buy_fresh). Принимает: ядро, направление, количество. Ошибки: как у costs_quote::fetch; Stop —
/// данные не доказуемо свежее.
pub async fn live_buy(core: &Core, d: &Direction, qty: Decimal) -> Result<Quote, Fail> {
    let q = Box::pin(super::costs_quote::fetch(core, d, false, qty, None)).await?;
    buy_fresh(core, d, &q).map_err(Fail::Stop)?;
    Ok(q)
}

/// Что делает: живая котировка продажи на B (вызывает costs_source.rs Live, в куче): после сдвига расчёта
/// (ride_shift.rs — одно чтение на расчёт: идущие сделки групп и худший случай решения 480) и по данным новее продаж
/// сделок групп (ride_trail.rs); граница «наша продажа уже в пуле / ещё нет» (ride_settled.rs) — до и после котировки.
/// Принимает: ядро, направление, количество, сдвиг. Ошибки: как у costs_quote::fetch; Stop — данные не доказуемо
/// свежее, своя продажа в полёте, одно состояние пары котировок не доказано (ride_pair.rs).
pub async fn live_sell(
    core: &Core,
    d: &Direction,
    qty: Decimal,
    sh: &super::ride_shift::Shift,
) -> Result<Quote, Fail> {
    let seq = super::ride_settled::before(core, d).map_err(Fail::Stop)?;
    let q = if sh.off > Decimal::ZERO {
        sell_after(core, d, qty, sh.off).await?
    } else {
        Box::pin(super::costs_quote::fetch(core, d, true, qty, None)).await?
    };
    super::ride_settled::after(core, d, seq).map_err(Fail::Stop)?;
    super::ride_trail::check(core, &CoinKey::of(&d.asset), &d.sell_on, &q).map_err(Fail::Stop)?;
    Ok(q)
}

/// Что делает: весь сдвиг продажи направления (ride_shift::read — группы и худший случай решения 480) — для
/// предпроверки DEX (precheck_dex_sell.rs). Принимает: ядро, направление. Возвращает: количество (0 — нет).
/// Ошибки: текст — как у ride_settled::worst.
pub fn ahead(core: &Core, d: &Direction) -> Result<Decimal, String> {
    super::ride_shift::read(core, d).map(|s| s.off)
}

/// Что делает: сколько монет идущих сделок группы будет продано на площадке продажи направления (остаток Deal.qty —
/// уже проданное отражает след продажи, ride_trail.rs). Решение 480: сделка с неизвестным исходом продажи на этой
/// площадке здесь не считается — её остаток в ahead (ride_settled::worst), без двойного счёта. Принимает: ядро,
/// направление. Возвращает: количество (0 — нет). Ошибок нет.
pub fn sell_ahead(core: &Core, d: &Direction) -> Decimal {
    let rides = group_rides(core, d);
    if rides.is_empty() {
        return Decimal::ZERO;
    }
    let deals = lk(&core.inner.deals);
    rides
        .iter()
        .filter(|r| r.sell_on == d.sell_on)
        .filter_map(|r| deals.get(&r.deal_id))
        .filter(|x| x.is_open() && !(x.sale_unsure && x.at == d.sell_on))
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
/// объём, ahead. Возвращает: marginal. Ошибки: как у costs_quote::fetch (на любой из двух котировок); Stop — одно
/// состояние не доказано (ride_pair.rs).
pub async fn sell_after(
    core: &Core,
    d: &Direction,
    qty: Decimal,
    off: Decimal,
) -> Result<Quote, Fail> {
    // Решение 480 (ревью 2): обе котировки — по одному состоянию площадки (ride_pair.rs).
    let (total, ahead) = Box::pin(super::ride_pair::live_pair(core, d, qty, off)).await?;
    Ok(marginal(&total, &ahead, qty, off))
}

/// Что делает: данные покупки новее покупок идущих сделок группы на той же площадке A (см. шапку). Принимает: ядро,
/// направление, живую котировку покупки. Ошибки: текст причины (данные не доказуемо свежее покупки идущей сделки).
pub fn buy_fresh(core: &Core, d: &Direction, q: &Quote) -> Result<(), String> {
    let rides = group_rides(core, d);
    if rides.is_empty() {
        return Ok(());
    }
    let kind = super::trade_ops::kind(core, &d.buy_on)?;
    let deals = lk(&core.inner.deals);
    for r in rides.iter().filter(|r| r.buy_on == d.buy_on) {
        let Some(b) = deals.get(&r.deal_id).and_then(|x| x.buy.as_ref()) else {
            continue;
        };
        let stale = |why: String| {
            Err(format!(
                "{}: данные покупки не доказуемо свежее покупки сделки {} — {why}",
                d.buy_on.0, r.deal_id
            ))
        };
        match (kind, &q.route, &b.request.route) {
            (VenueKind::Cex, ..) => {
                if q.at <= b.finished_at {
                    return stale(format!(
                        "стакан получен {}, её ордер кончился {}; ждём кадр после неё",
                        q.at, b.finished_at
                    ));
                }
            }
            (VenueKind::Dex, Some(mine), Some(theirs)) => {
                let theirs = super::ride_trail::sites(theirs);
                if !super::ride_trail::sites(mine)
                    .iter()
                    .any(|p| theirs.contains(p))
                {
                    continue;
                }
                match b.chain.as_ref().and_then(|c| c.block) {
                    Some(bl) if mine.block >= bl => {}
                    Some(bl) => {
                        return stale(format!(
                            "маршрут по блоку {}, её покупка в блоке {bl} в тех же пулах",
                            mine.block
                        ));
                    }
                    None => return stale("блок её покупки неизвестен".into()),
                }
            }
            (VenueKind::Dex, None, _) => {
                return stale("у котировки нет маршрута и блока (агрегатор)".into());
            }
            (VenueKind::Dex, Some(mine), None) => {
                // Её покупка шла без маршрута (агрегатор) — её пулов не знаем: маршрут не раньше её блока.
                match b.chain.as_ref().and_then(|c| c.block) {
                    Some(bl) if mine.block >= bl => {}
                    _ => return stale("её покупка без маршрута, блок не доказуемо позже".into()),
                }
            }
        }
    }
    Ok(())
}
