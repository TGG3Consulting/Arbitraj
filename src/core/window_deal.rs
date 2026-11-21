//! Файл: core/window_deal.rs
//! Что делает: решение 245 (часть А, Тигран 09.11: согласовано) — у окна (решение 125), где по направлению идёт НАША
//! сделка биржа ↔ DEX, точки цены DEX на ОБЪЁМ СДЕЛКИ (решение 241 пишет объём сравнения тика). Сделка под наблюдением,
//! пока её сторона DEX ещё впереди: биржа → DEX — пока сделка открыта (продажа на DEX); DEX → биржа — только до факта
//! покупки на DEX (после покупки цена DEX на итог сделки не влияет, а пулы уже сдвинуты нашим же обменом).
//! Объём: биржа → DEX — монеты на DEX (deal.qty, «факт»), пока их там нет — плановый объём продажи (Calc.sell_qty,
//! «план»); DEX → биржа — плановый объём покупки (Calc.qty, «план»).
//! Когда точка: первая — сразу после записи сделки в учёт; дальше — на событие ЛЮБОГО пула маршрута сделки (пулы
//! маршрута плана и всех прошлых котировок этой сделки, включая промежуточные пулы графа без монеты сделки) и не чаще
//! window.dex_price_min_ms; событие внутри интервала не теряется (отметка «есть изменение» ждёт конца интервала).
//! Котировка — котировка адаптера DEX кошельком сделки (Venue::quote, класс Quote). Дополнение 245 п. 3 (Тигран 09.11:
//! «маршрут важен»): тем же путём, что финальная котировка кандидата и исполнение (gateway::quote_stage::final_scope —
//! адаптер выбирает: свой сборщик / KyberSwap при exec.via_kyber / «лучший из двух» при exec.dex_best_of); путь — в
//! точке (DealSize.path, winner, pick; window_deal_run.rs). Свой сборщик — по состоянию пулов в памяти, как котировка
//! тика (venues/bsc_dex/quote/mod.rs: decimals, разрешения и газ перевода чаевых — из памяти после первого чтения, цена
//! газа — один запрос на блок на всех); KyberSwap — внешний API с темпом kyber.rps (общая очередь с исполнением), поэтому
//! при window.dex_price_min_ms = 0 KyberSwap для точек не спрашивается (path «own_rate»). Своя задача (run): тик
//! только ставит отметку под коротким замком; котировка и строка окна — здесь, сообщением задаче окон (window_run.rs).
//! Сбой котировки — в журнал при смене причины по сделке и на страницу окна (errors).
//! Здесь — кто под наблюдением и отметки; котировка и точка — window_deal_run.rs.
//! Кто вызывает: core/ledger.rs (saved), core/tick.rs (touched), core/window_view.rs (errors), window_deal_run.rs, тесты.
//! От чего зависит: window_dex.rs (dex_side), coin_key.rs, deal.rs.
//! Настройки: window.dex_price, window.dex_price_min_ms (те же, что у решения 241).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::core::types::{Asset, DexRoute, Direction, PoolRef, Side, VenueId};

use super::coin_key::CoinKey;
use super::deal::Deal;
use super::engine::{Core, lk};
use super::window_dex::dex_side;

/// Пометка точки на объём сделки (поле `deal` точки цены DEX).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DealSize {
    /// id сделки.
    pub id: String,
    /// Откуда объём: «fact» — монеты на DEX (deal.qty), «plan» — план сделки.
    pub qty_from: String,
    /// Запрошенный объём котировки, монет (исполнимый — DexPoint.qty).
    pub asked: Decimal,
    /// Дополнение 245 п. 3 (Тигран 09.11: «маршрут важен»): каким путём посчитана точка — window_deal_run.rs path_of:
    /// «own» — свой сборщик (exec.via_kyber нет — как исполнение), «kyber» — KyberSwap (exec.via_kyber), «kyber_no_route»
    /// — exec.via_kyber, у KyberSwap нет маршрута, свой (как откат исполнения), «best_of» — «лучший из двух»
    /// (exec.dex_best_of; кто выиграл — winner, почему — pick), «own_rate» — exec.via_kyber, но KyberSwap не
    /// спрашивался: window.dex_price_min_ms = 0 (без предела частоты запросов к внешнему API). None — точка до дополнения
    /// (своя котировка без KyberSwap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Кто дал котировку: «own» — свой сборщик, «kyber» — KyberSwap. None — точка до дополнения.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner: Option<String>,
    /// «Лучший из двух»: текст выбора адаптера (Quote.pick). None — выбора не было.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<String>,
}

/// Сделка под наблюдением.
#[derive(Debug, Clone)]
pub(super) struct Entry {
    pub(super) direction: Direction,
    pub(super) venue: VenueId,
    pub(super) side: Side,
    pub(super) pools: HashSet<PoolRef>,
    /// Пул маршрута менялся после прошлой точки (или точки ещё не было).
    pub(super) dirty: bool,
    pub(super) last: Option<Instant>,
    pub(super) err: Option<String>,
}

/// Сделки под наблюдением (поле Inner::window_deals).
#[derive(Default)]
pub struct Watch {
    pub(super) deals: Mutex<HashMap<String, Entry>>,
    /// Сколько сделок под наблюдением — тик без них замок не берёт.
    n: AtomicUsize,
    pub(super) kick: Notify,
}

/// Что делает: сделка записана в учёт — взять под наблюдение, обновить или снять (см. шапку). Только память.
/// Принимает: ядро, сделку. Ошибок нет.
pub fn saved(core: &Core, deal: &Deal) {
    let w = &core.inner.window_deals;
    let side = dex_side(core, &deal.direction);
    let keep = match &side {
        Some((_, Side::Sell)) => deal.is_open(),
        Some((_, Side::Buy)) => deal.is_open() && deal.buy.is_none(),
        None => false,
    };
    let mut m = lk(&w.deals);
    match side.filter(|_| keep) {
        None => {
            m.remove(&deal.id);
        }
        Some((venue, side)) if !m.contains_key(&deal.id) => {
            let pools = route(deal, side).iter().flat_map(refs).collect();
            let e = Entry {
                direction: deal.direction.clone(),
                venue,
                side,
                pools,
                dirty: true,
                last: None,
                err: None,
            };
            m.insert(deal.id.clone(), e);
            w.kick.notify_one();
        }
        Some(_) => {}
    }
    w.n.store(m.len(), Ordering::Release);
}

/// Что делает: событие пула (tick.rs) — отметка у сделок, в маршруте которых этот пул. Только память. Ошибок нет.
pub fn touched(core: &Core, venue: &VenueId, pool: &PoolRef) {
    let w = &core.inner.window_deals;
    if w.n.load(Ordering::Acquire) == 0 {
        return;
    }
    let mut hit = false;
    for e in lk(&w.deals).values_mut() {
        if e.venue == *venue && e.pools.contains(pool) {
            e.dirty = true;
            hit = true;
        }
    }
    if hit {
        w.kick.notify_one();
    }
}

/// Последняя ошибка котировки по сделкам монеты под наблюдением (страница окна). Принимает: ядро, монету.
/// Возвращает: id сделки → причина. Ошибок нет.
pub fn errors(core: &Core, asset: &Asset) -> HashMap<String, String> {
    let key = CoinKey::of(asset);
    lk(&core.inner.window_deals.deals)
        .iter()
        .filter(|(_, e)| CoinKey::of(&e.direction.asset) == key)
        .filter_map(|(id, e)| Some((id.clone(), e.err.clone()?)))
        .collect()
}

/// Сделки с отметкой «пул маршрута менялся» (или первая точка), ждущие котировки, по id (проверка и тесты). Ошибок нет.
pub fn pending(core: &Core) -> Vec<String> {
    let mut v: Vec<String> = lk(&core.inner.window_deals.deals)
        .iter()
        .filter(|(_, e)| e.dirty)
        .map(|(id, _)| id.clone())
        .collect();
    v.sort();
    v
}

/// Маршрут стороны DEX из плана сделки (Calc.sell_quote / buy_quote). Ошибок нет: нет — None.
fn route(deal: &Deal, side: Side) -> Option<DexRoute> {
    let k = if side == Side::Sell {
        "sell_quote"
    } else {
        "buy_quote"
    };
    serde_json::from_value(deal.plan[k]["route"].clone()).ok()
}

/// Пулы маршрута. Ошибок нет.
pub(super) fn refs(r: &DexRoute) -> impl Iterator<Item = PoolRef> + '_ {
    r.legs.iter().flat_map(|l| &l.hops).map(|h| h.pool.clone())
}
