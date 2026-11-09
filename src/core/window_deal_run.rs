//! Файл: core/window_deal_run.rs
//! Что делает: решение 245 (часть А) — задача точек цены DEX на объём нашей сделки (кто под наблюдением и когда —
//! core/window_deal.rs): по сделкам с отметкой «пул маршрута менялся», у которых прошёл window.dex_price_min_ms, —
//! котировка стороны DEX на объём сделки кошельком сделки (Venue::quote — память пулов, без запросов к узлу сверх
//! котировки тика, см. шапку window_deal.rs) и точка в окно направления (поле `deal`: id, план/факт, запрошенный
//! объём) сообщением задаче окон. Окна направления нет — котировки нет. Сбой — причина у сделки и запись в журнал на
//! смену причины.
//! Кто вызывает: core/engine.rs (run), тесты (step, write, qty_of).
//! От чего зависит: window_deal.rs (Watch, Entry), window.rs (dex_slot), window_dex.rs (cfg, pools, DexPoint),
//! window_run.rs (send), dex_account.rs (кошелёк сделки), venue_ticker.rs.
//! Настройки: window.dex_price, window.dex_price_min_ms.

use rust_decimal::Decimal;
use tokio::time::{Duration, Instant, sleep_until};

use crate::core::types::{Direction, Quote, Side, VenueId};
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::{Core, lk};
use super::window_deal::{DealSize, Entry, refs};
use super::window_dex::{DexPoint, cfg, pools};
use super::window_run::{WinMsg, send};

/// Задача точек на объём сделки: просыпается от отметок и к концу интервала. Работает, пока жив процесс.
pub async fn run(core: Core) {
    loop {
        let wake = step(&core).await;
        let kick = core.inner.window_deals.kick.notified();
        match wake {
            Some(t) => tokio::select! { _ = kick => {}, _ = sleep_until(t) => {} },
            None => kick.await,
        }
    }
}

/// Что делает: котировки по сделкам с отметкой, у которых прошёл интервал, и точки в окна. Принимает: ядро.
/// Возвращает: когда проснуться к ближайшей отложенной отметке; None — ждать отметку. Ошибок нет.
pub async fn step(core: &Core) -> Option<Instant> {
    let min = Duration::from_millis(cfg(&core.inner.deps.settings).ok()?);
    let now = Instant::now();
    let mut next = None;
    let due: Vec<(String, Entry)> = {
        let mut m = lk(&core.inner.window_deals.deals);
        let mut w = lk(&core.inner.windows);
        let mut out = vec![];
        for (id, e) in m.iter_mut().filter(|(_, e)| e.dirty) {
            if w.dex_slot(&e.direction).is_none() {
                continue;
            }
            match e.last.map(|l| l + min).filter(|t| *t > now) {
                Some(t) => next = Some(next.map_or(t, |n: Instant| n.min(t))),
                None => {
                    (e.dirty, e.last) = (false, Some(now));
                    out.push((id.clone(), e.clone()));
                }
            }
        }
        out
    };
    for (id, e) in due {
        let Some(deal) = lk(&core.inner.deals).get(&id).cloned() else {
            continue;
        };
        let (qty, from) = qty_of(&deal, e.side, &e.venue);
        let r = match qty {
            Some(q) if q > Decimal::ZERO => quote(core, &deal, &e, q).await,
            _ => Err("объёма сделки нет (план без количества)".into()),
        };
        match r {
            Ok(q) => {
                let size = DealSize {
                    id: id.clone(),
                    qty_from: from.into(),
                    asked: qty.unwrap_or_default(),
                };
                write(core, &e.direction, e.side, &q, size);
                if let Some(x) = lk(&core.inner.window_deals.deals).get_mut(&id) {
                    x.pools.extend(q.route.iter().flat_map(refs));
                    x.err = None;
                }
            }
            Err(why) => failed(core, &id, why).await,
        }
    }
    next
}

/// Что делает: точка на объём сделки — в окно направления (счётчик точек, строка файла задачей окон). Принимает: ядро,
/// направление, сторону DEX, котировку, пометку сделки. Возвращает: записана ли (окно направления открыто).
pub fn write(core: &Core, d: &Direction, side: Side, q: &Quote, size: DealSize) -> bool {
    let point = DexPoint {
        at: q.at,
        buy_on: d.buy_on.clone(),
        sell_on: d.sell_on.clone(),
        venue: q.venue.clone(),
        side,
        price: q.avg_price,
        qty: q.fillable_qty,
        block: q.route.as_ref().map(|r| r.block),
        pools: pools(q),
        deal: Some(size),
    };
    let id = {
        let mut w = lk(&core.inner.windows);
        let Some((info, _)) = w.dex_slot(d) else {
            return false;
        };
        info.dex_points += 1;
        info.id.clone()
    };
    let point = Box::new(point);
    send(core, WinMsg::Dex { id, point });
    true
}

/// Что делает: объём котировки и откуда он (шапка window_deal.rs): биржа → DEX — монеты на DEX («fact»), пока их там нет —
/// Calc.sell_qty плана («plan»); DEX → биржа — Calc.qty плана. Принимает: сделку, сторону DEX, площадку DEX.
/// Возвращает: (объём, «fact» / «plan»); объёма нет в плане — None. Ошибок нет.
pub fn qty_of(deal: &Deal, side: Side, venue: &VenueId) -> (Option<Decimal>, &'static str) {
    let plan = |k: &str| serde_json::from_value::<Decimal>(deal.plan[k].clone()).ok();
    match side {
        Side::Sell if deal.at == *venue && deal.qty > Decimal::ZERO => (Some(deal.qty), "fact"),
        Side::Sell => (plan("sell_qty"), "plan"),
        Side::Buy => (plan("qty"), "plan"),
    }
}

/// Котировка стороны DEX кошельком сделки (решение 132). Ошибки: текст — нет кошелька, площадка, котировка.
async fn quote(core: &Core, deal: &Deal, e: &Entry, qty: Decimal) -> Result<Quote, String> {
    let v = super::dex_account::venue_of(core, deal, &e.venue).await?;
    let a = &deal.direction.asset;
    let on = super::venue_ticker::on_venue(core, a, &e.venue);
    let q = v.quote(&on, e.side, qty).await.map_err(|x| x.message)?;
    Ok(super::venue_ticker::quote_back(core, a, &e.venue, q))
}

/// Сбой котировки: причина у сделки (страница окна), в журнал — при смене причины. Ошибок нет.
async fn failed(core: &Core, id: &str, why: String) {
    let changed = match lk(&core.inner.window_deals.deals).get_mut(id) {
        Some(x) if x.err.as_deref() != Some(&why) => {
            x.err = Some(why.clone());
            true
        }
        _ => false,
    };
    if changed {
        let text = format!("окно: цена DEX на объём сделки {id} не посчитана: {why}");
        core.inner
            .deps
            .log(Level::Warn, "core.window", None, text, None)
            .await;
    }
}
