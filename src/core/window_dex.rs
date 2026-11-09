//! Файл: core/window_dex.rs
//! Что делает: решение 241 (Тигран 09.11) — цена пула DEX в окне (решение 125). Окно пишет стакан биржи; сторона DEX
//! направления (биржа → DEX — продажа на DEX, DEX → биржа — покупка на DEX) пишется точками цены строкой файла окна
//! `{"type":"dex","point":{…}}`, чтобы движение цены DEX между решением и продажей было видно по данным бота.
//! Источник точки — котировка DEX, которую тик УЖЕ посчитал для этого направления (core/prices.rs for_direction —
//! пересчёт монеты на каждое событие её пулов, tick.rs): средняя цена на объём сравнения тика (Quote::avg_price,
//! проскальзывание и затраты «в цене» — комиссия пула, налог — внутри), её объём, блок и пулы маршрута. Новых
//! котировок и запросов к узлу нет. Почему не «цена пула» по состоянию: в ядре её нет — у пула в ядре нет decimals
//! токенов, математика видов пулов — только в адаптере (core/types/pool.rs, core/prices.rs).
//! Когда пишется точка: окно монеты открыто, направление входило в него, window.dex_price = да, задана
//! window.dex_price_min_ms; первая точка направления в окне — сразу; дальше — только если пулы монеты на этой DEX
//! менялись (поколение пулов токена монеты, core/quote_cache.rs — растёт на каждом событии пула, tick.rs; поколение
//! читается ДО котировки) и с прошлой точки прошло не меньше window.dex_price_min_ms (0 — каждое изменение). Изменение
//! внутри интервала не теряется: поколение уже другое — точка пишется на первом пересчёте после интервала. Монета без
//! адреса контракта (поколения нет) — точка при смене цены или блока маршрута.
//! Не видно: изменение пулов маршрута БЕЗ токена монеты (например, промежуточная WBNB/USDT) пересчёт монеты не толкает
//! (tick.rs: монеты по токенам пула) — такое движение попадёт в точку только со следующим пересчётом монеты.
//! Только память и сообщение задаче окон (window_run.rs send): тик ничего не ждёт; пишет строку window_worker.rs.
//! Решение 245: у направления с нашей открытой сделкой — ещё точки на объём сделки (core/window_deal.rs, поле `deal`
//! точки); точки этого файла — объём тика (`deal` пусто).
//! Кто вызывает: core/prices.rs (generation, note), core/window_view.rs (cfg — причина на странице окна),
//! core/window_deal.rs (cfg, dex_side, pools, DexPoint), тесты.
//! От чего зависит: window.rs (dex_slot), window_run.rs (WinMsg, send), quote_cache.rs, inner.kinds, settings.
//! Настройки: window.dex_price, window.dex_price_min_ms.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::{Direction, PoolRef, Quote, Side, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;
use crate::settings::store::Store;

use super::engine::{Core, lk, rd};
use super::window_run::{WinMsg, send};

/// Точка цены DEX в окне — строка файла и элемент `dex` страницы окна.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DexPoint {
    /// Когда посчитана котировка (Quote::at).
    pub at: DateTime<Utc>,
    /// Направление, на котором посчитана: площадка покупки и продажи.
    pub buy_on: VenueId,
    pub sell_on: VenueId,
    /// Площадка DEX и сторона на ней: Sell — биржа → DEX (продаём на DEX), Buy — DEX → биржа.
    pub venue: VenueId,
    pub side: Side,
    /// Средняя цена котировки, USDT за монету (Quote::avg_price).
    pub price: Decimal,
    /// Объём котировки, монет (Quote::fillable_qty) — объём сравнения тика, не объём сделки.
    pub qty: Decimal,
    /// Блок, по состоянию которого посчитан маршрут; None — маршрута в котировке нет.
    pub block: Option<u64>,
    /// Пулы маршрута: адрес контракта; V4 — менеджер:id; пара общего контракта — контракт:монета.
    pub pools: Vec<String>,
    /// Решение 245: точка на объём нашей сделки (window_deal.rs); None — объём сравнения тика (решение 241).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deal: Option<super::window_deal::DealSize>,
}

/// Последняя записанная точка направления: поколение пулов, блок, цена, когда записана.
#[derive(Debug, Clone)]
pub struct Mark {
    generation: Option<u64>,
    block: Option<u64>,
    price: Decimal,
    at: DateTime<Utc>,
}

/// Памятка окна: направление → последняя точка (живёт в открытом окне, window.rs).
pub type Marks = HashMap<Direction, Mark>;

/// Что делает: настройки точек. Принимает: хранилище. Возвращает: window.dex_price_min_ms.
/// Ошибки: текст — window.dex_price = нет или не задана настройка (причина на странице окна).
pub fn cfg(s: &Store) -> Result<u64, String> {
    if !s
        .flag("window.dex_price", At::default())
        .map_err(|e| e.message)?
    {
        return Err("window.dex_price = нет".into());
    }
    s.integer("window.dex_price_min_ms", At::default())
        .map_err(|e| e.message)
}

/// Площадка DEX направления и сторона на ней (по виду площадки из шлюза); None — DEX в направлении нет. Ошибок нет.
pub(super) fn dex_side(core: &Core, d: &Direction) -> Option<(VenueId, Side)> {
    let kinds = rd(&core.inner.kinds);
    let dex = |v: &VenueId| kinds.iter().any(|(id, k)| id == v && *k == VenueKind::Dex);
    if dex(&d.sell_on) {
        Some((d.sell_on.clone(), Side::Sell))
    } else if dex(&d.buy_on) {
        Some((d.buy_on.clone(), Side::Buy))
    } else {
        None
    }
}

/// Что делает: поколение пулов токена монеты на DEX направления — вызывать ДО котировки. Принимает: ядро,
/// направление. Возвращает: поколение; None — окна монеты с этим направлением нет, DEX нет или у монеты нет адреса.
/// Ошибок нет.
pub fn generation(core: &Core, d: &Direction) -> Option<u64> {
    lk(&core.inner.windows).dex_slot(d)?;
    let (venue, _) = dex_side(core, d)?;
    let token = d.asset.bsc_contract?;
    Some(lk(&core.inner.quote_cache).generation(&venue, token))
}

/// Что делает: котировки тика направления посчитаны — точка цены DEX в окно, если пора (см. шапку). Только память и
/// сообщение. Принимает: ядро, направление, котировки покупки и продажи, поколение пулов (generation до котировки).
/// Ошибок нет: окна нет, настройки не заданы — ничего.
pub fn note(core: &Core, d: &Direction, qa: &Quote, qb: &Quote, generation: Option<u64>) {
    if lk(&core.inner.windows).dex_slot(d).is_none() {
        return;
    }
    let Some((venue, side)) = dex_side(core, d) else {
        return;
    };
    let Ok(min_ms) = cfg(&core.inner.deps.settings) else {
        return;
    };
    let q = if side == Side::Buy { qa } else { qb };
    let point = DexPoint {
        at: q.at,
        buy_on: d.buy_on.clone(),
        sell_on: d.sell_on.clone(),
        venue,
        side,
        price: q.avg_price,
        qty: q.fillable_qty,
        block: q.route.as_ref().map(|r| r.block),
        pools: pools(q),
        deal: None,
    };
    let now = Utc::now();
    let id = {
        let mut w = lk(&core.inner.windows);
        let Some((info, marks)) = w.dex_slot(d) else {
            return;
        };
        if !due(marks.get(d), generation, &point, min_ms, now) {
            return;
        }
        let mark = Mark {
            generation,
            block: point.block,
            price: point.price,
            at: now,
        };
        marks.insert(d.clone(), mark);
        info.dex_points += 1;
        info.id.clone()
    };
    let point = Box::new(point);
    send(core, WinMsg::Dex { id, point });
}

/// Пора ли точка: первой нет — да; иначе пулы менялись (поколение; без поколения — цена или блок) и прошло не меньше
/// min_ms. Ошибок нет.
fn due(
    last: Option<&Mark>,
    generation: Option<u64>,
    p: &DexPoint,
    min_ms: u64,
    now: DateTime<Utc>,
) -> bool {
    let Some(m) = last else { return true };
    let changed = match (generation, m.generation) {
        (Some(g), Some(l)) => g != l,
        _ => p.block != m.block || p.price != m.price,
    };
    changed && (now - m.at).num_milliseconds() >= i64::try_from(min_ms).unwrap_or(i64::MAX)
}

/// Пулы маршрута котировки без повторов, по порядку долей и шагов. Ошибок нет.
pub(super) fn pools(q: &Quote) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let hops = q.route.iter().flat_map(|r| &r.legs).flat_map(|l| &l.hops);
    for h in hops {
        let t = match &h.pool {
            PoolRef::Contract(a) => format!("{a:#x}"),
            PoolRef::Singleton { manager, id } => format!("{manager:#x}:{id:#x}"),
            PoolRef::Member { contract, coin } => format!("{contract:#x}:{coin:#x}"),
        };
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}
