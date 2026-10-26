//! Файл: core/card.rs
//! Что делает: полная карточка сигнала (решение 74) для «Живого экрана» и Telegram: монета (тикер и контракт BSC),
//! где покупать и где продавать, количество и средние цены, стакан биржи (лучшая цена и USDT в полосе — card_book.rs),
//! пулы маршрута DEX (средняя цена обмена и глубина по стороне сделки — из Venue::pool_depths, решение 73, заново не
//! считается), максимальный объём и прибыль без бюджета (card_max.rs) и прибыль на бюджете, ссылки (card_links.rs).
//! Ни одна часть карточки не мешает решению по кандидату: чего нет — причина в своём поле.
//! Кто вызывает: core/candidates.rs (прошедший кандидат), core/episode_run.rs (пересчёт в эпизоде).
//! От чего зависит: card_book.rs, card_links.rs, card_max.rs, size_max.rs, costs.rs, engine.rs, trade_ops.rs (вид площадки),
//! шлюз (pool_depths), settings.
//! Настройки: dex.depth_band_pct (полоса стакана — площадки-DEX направления), card.max_search_usdt (через card_max.rs),
//! venue.pair_url_template, venue.pool_url_template, venue.quote_asset (через card_links.rs).

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::Serialize;

use crate::core::types::{DexRoute, PoolKind, Side, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::card_book::{self, BookSide};
use super::card_links::{self, Link};
use super::costs::Calc;
use super::engine::{Core, rd};
use super::size_max::MaxVolume;

/// Пул маршрута DEX в карточке.
#[derive(Debug, Clone, Serialize)]
pub struct PoolSide {
    pub venue: String,
    /// Адрес пула (V2/V3) или PoolId (V4).
    pub pool: String,
    pub family: String,
    pub kind: PoolKind,
    /// Сторона сделки на DEX: Buy — глубина покупки, Sell — продажи (решение 73).
    pub side: Side,
    /// Глубина по стороне в полосе, USDT; полоса, %; глубокое ли направление; полоса прочитана целиком.
    pub depth_usd: Option<Decimal>,
    pub band_pct: Option<Decimal>,
    pub deep: bool,
    pub full: bool,
    /// Блок, по которому посчитан маршрут.
    pub block: u64,
    /// Почему глубины нет (адаптер не ответил, не посчитана).
    pub error: Option<String>,
}

/// Карточка сигнала.
#[derive(Debug, Clone, Serialize)]
pub struct Card {
    /// Сухой режим (решение 74): денег нет, исполнитель не вызывался.
    pub dry: bool,
    pub ticker: String,
    /// Адрес контракта монеты в BSC (0x…).
    pub contract: Option<String>,
    pub buy_on: String,
    pub sell_on: String,
    /// Сколько купить на A, средняя цена; сколько продать на B, средняя цена.
    pub qty: Decimal,
    pub buy_avg: Decimal,
    pub sell_qty: Decimal,
    pub sell_avg: Decimal,
    /// Бюджет (реальный или сухой) и прибыль на нём.
    pub budget: Option<Decimal>,
    pub profit_usd: Decimal,
    pub profit_pct: Decimal,
    pub books: Vec<BookSide>,
    pub pools: Vec<PoolSide>,
    /// Без бюджета; Err — почему не посчитано.
    pub max: Result<MaxVolume, String>,
    pub links: Vec<Link>,
}

/// Что делает: собирает карточку. Принимает: ядро, расчёт прошедшего кандидата, сухой ли режим.
/// Возвращает: Card. Ошибок нет — причины в полях.
pub async fn build(core: &Core, calc: &Calc, dry: bool) -> Card {
    let s = &core.inner.deps.settings;
    let d = &calc.direction;
    let kind = |v: &VenueId| super::trade_ops::kind(core, v).ok();
    let dex = [&d.buy_on, &d.sell_on]
        .into_iter()
        .find(|v| kind(v) == Some(VenueKind::Dex))
        .cloned();
    let band = || -> Result<Decimal, String> {
        let v = dex
            .as_ref()
            .ok_or("у направления нет DEX — полоса dex.depth_band_pct не задаётся")?;
        let at = At {
            venue: Some(&v.0),
            coin: None,
        };
        s.decimal("dex.depth_band_pct", at).map_err(|e| e.message)
    };
    let (mut books, mut pools, mut links) = (vec![], vec![], vec![]);
    for (v, side, route) in [
        (&d.buy_on, Side::Buy, &calc.buy_quote.route),
        (&d.sell_on, Side::Sell, &calc.sell_quote.route),
    ] {
        match kind(v) {
            Some(VenueKind::Cex) => {
                let book = rd(&core.inner.market).book(v, &d.asset.ticker).cloned();
                let levels = book.as_ref().map(|b| match side {
                    Side::Buy => b.asks.as_slice(),
                    Side::Sell => b.bids.as_slice(),
                });
                books.push(card_book::side(&v.0, side, levels, band()));
                links.push(card_links::pair(s, &v.0, &d.asset.ticker));
            }
            Some(VenueKind::Dex) => {
                let got = route_pools(core, v, side, route.as_ref()).await;
                for p in &got {
                    if let Some(r) = pool_ref(route.as_ref(), &p.pool) {
                        links.push(card_links::pool(s, &v.0, r));
                    }
                }
                pools.extend(got);
            }
            None => {}
        }
    }
    Card {
        dry,
        ticker: d.asset.ticker.clone(),
        contract: d.asset.bsc_contract.map(|a| format!("{a:#x}")),
        buy_on: d.buy_on.0.clone(),
        sell_on: d.sell_on.0.clone(),
        qty: calc.qty,
        buy_avg: calc.buy_avg,
        sell_qty: calc.sell_qty,
        sell_avg: calc.sell_avg,
        budget: calc.size.as_ref().map(|z| z.budget),
        profit_usd: calc.profit_usd,
        profit_pct: calc.profit_pct,
        books,
        pools,
        max: super::card_max::compute(core, calc).await,
        links,
    }
}

/// Пулы маршрута с глубиной по стороне из адаптера (pool_depths). Принимает: ядро, площадку, сторону, маршрут.
/// Возвращает: строки пулов (нет маршрута — пусто). Ошибок нет — причина в error каждого пула.
async fn route_pools(
    core: &Core,
    v: &VenueId,
    side: Side,
    route: Option<&DexRoute>,
) -> Vec<PoolSide> {
    let Some(route) = route else { return vec![] };
    let mut uniq = BTreeMap::new();
    for hop in route.legs.iter().flat_map(|l| &l.hops) {
        uniq.entry(card_links::pool_id(&hop.pool)).or_insert(hop);
    }
    let depths = match core.inner.deps.gw.get(v).await {
        Ok(a) => a.pool_depths().await.map_err(|e| e.message),
        Err(e) => Err(e.message),
    };
    uniq.into_iter()
        .map(|(id, hop)| {
            let dp = depths
                .as_ref()
                .ok()
                .and_then(|all| all.iter().find(|p| p.venue == *v && p.pool == hop.pool));
            let (depth, deep) = match (dp, side) {
                (Some(p), Side::Buy) => (p.buy_usd, p.deep_buy),
                (Some(p), Side::Sell) => (p.sell_usd, p.deep_sell),
                (None, _) => (None, false),
            };
            let error = match (&depths, dp) {
                (Err(e), _) => Some(format!("глубина: {e}")),
                (Ok(_), None) => Some("адаптер не знает этот пул".into()),
                (Ok(_), Some(p)) => p.depth_error.clone(),
            };
            PoolSide {
                venue: v.0.clone(),
                pool: id,
                family: hop.family.clone(),
                kind: hop.kind,
                side,
                depth_usd: depth,
                band_pct: dp.and_then(|p| p.band_pct),
                deep,
                full: dp.is_some_and(|p| p.full),
                block: route.block,
                error,
            }
        })
        .collect()
}

/// Ссылка на пул маршрута по его строковому id. Ошибок нет.
fn pool_ref<'a>(route: Option<&'a DexRoute>, id: &str) -> Option<&'a crate::core::types::PoolRef> {
    route?
        .legs
        .iter()
        .flat_map(|l| &l.hops)
        .map(|h| &h.pool)
        .find(|p| card_links::pool_id(p) == id)
}
