//! Файл: core/prices.rs
//! Что делает: цены направления для грубого спреда (шаг 3): котировки шлюза на объём сравнения
//! на обеих сторонах. Объём сравнения — МАКСИМАЛЬНО вливаемый (меньшая из глубин обеих сторон), но не
//! выше потолка money.max_per_trade_usd площадки A / опорная цена (решение 87). Потолок — это предел,
//! а не обязательный размер: грубый спред НЕ требует заполнить его целиком, иначе тонкие пулы/стаканы
//! отбрасывались как NoPrice ещё на шаге 3 и не доходили до подбора объёма (решение 66), который сам
//! ужал бы вход до прибыльного. Опорная цена — лучший ask монеты в стакане любой биржи (market_state).
//! Почему котировка, а не только лучшая цена: у пула DEX в стыке нет «лучшей цены» (нужны decimals
//! токенов — запрос в Issue #8), а котировка адаптера заодно проверяет свежесть данных:
//! устарели — ошибка, сигнала нет («не торгуем вслепую»).
//! Решение 109: на тике котировка DEX может браться из памяти (quote_cache.rs) — пулы не менялись, тот же объём,
//! не старше signal.dex_quote_cache_ms.
//! Кто вызывает: core/evaluate.rs на каждый тик монеты.
//! От чего зависит: gateway (реестр, котировка), market_state.rs, quote_cache.rs, settings.
//! Настройки: money.max_per_trade_usd (по площадке A), venue.quote_asset (по площадке), signal.dex_quote_cache_ms.

use rust_decimal::Decimal;

use crate::core::types::{CostKind, Direction, Quote, Side};
use crate::settings::read::At;

use super::deps::Deps;
use super::engine::{Core, lk, rd};
use super::signal::Prices;
use crate::gateway::contract::VenueKind;

/// Котировки обеих сторон направления на объём qty.
/// Принимает: зависимости, направление, объём. Возвращает: (покупка на A, продажа на B).
/// Ошибки: текст причины (площадка не подключена, нет данных, данные устарели).
pub async fn quotes(deps: &Deps, dir: &Direction, qty: Decimal) -> Result<(Quote, Quote), String> {
    let a = deps.gw.get(&dir.buy_on).await.map_err(|e| e.message)?;
    let b = deps.gw.get(&dir.sell_on).await.map_err(|e| e.message)?;
    let (qa, qb) = tokio::join!(
        a.quote(&dir.asset, Side::Buy, qty),
        b.quote(&dir.asset, Side::Sell, qty)
    );
    let qa = qa.map_err(|e| format!("{}: {}", dir.buy_on.0, e.message))?;
    let qb = qb.map_err(|e| format!("{}: {}", dir.sell_on.0, e.message))?;
    Ok((qa, qb))
}

/// Котировки обеих сторон на тике: сторона DEX — из памяти, если годится (решение 109). Ошибки: как у quotes.
async fn tick_quotes(core: &Core, dir: &Direction, qty: Decimal) -> Result<(Quote, Quote), String> {
    let (qa, qb) = tokio::join!(
        side_quote(core, dir, Side::Buy, qty),
        side_quote(core, dir, Side::Sell, qty)
    );
    Ok((qa?, qb?))
}

/// Одна сторона на тике. DEX и задан signal.dex_quote_cache_ms > 0 — сначала память, потом шлюз (и в память).
async fn side_quote(
    core: &Core,
    dir: &Direction,
    side: Side,
    qty: Decimal,
) -> Result<Quote, String> {
    let deps = &core.inner.deps;
    let venue = if side == Side::Buy {
        &dir.buy_on
    } else {
        &dir.sell_on
    };
    let dex = rd(&core.inner.kinds)
        .iter()
        .any(|(v, k)| v == venue && *k == VenueKind::Dex);
    let ttl = deps
        .settings
        .integer("signal.dex_quote_cache_ms", At::default())
        .ok()
        .filter(|ms| *ms > 0)
        .map(std::time::Duration::from_millis);
    let cached = match (dex, ttl, dir.asset.bsc_contract) {
        (true, Some(ttl), Some(token)) => {
            let key = (venue.clone(), dir.asset.ticker.clone(), side == Side::Buy);
            let cache = lk(&core.inner.quote_cache);
            let generation = cache.generation(venue, token);
            match cache.get(&key, qty, generation, ttl) {
                Some(q) => return Ok(q),
                None => Some((key, generation)),
            }
        }
        _ => None,
    };
    let a = deps.gw.get(venue).await.map_err(|e| e.message)?;
    let q = a
        .quote(&dir.asset, side, qty)
        .await
        .map_err(|e| format!("{}: {}", venue.0, e.message))?;
    if let Some((key, generation)) = cached {
        lk(&core.inner.quote_cache).put(key, qty, q.clone(), generation);
    }
    Ok(q)
}

/// Цены направления для сигнала.
/// Принимает: зависимости, направление, опорную цену монеты (лучший ask биржи; None — стакана нет).
/// Возвращает: Prices на максимально вливаемом объёме (≤ потолка). Ошибки: текст причины (нет опорной
/// цены, не задан лимит сделки, нет котировки, нет исполнимого объёма ни на сколько — глубина 0,
/// комиссия в валюте, которую не перевести без convert.rs).
pub async fn for_direction(
    core: &Core,
    dir: &Direction,
    ref_price: Option<Decimal>,
) -> Result<Prices, String> {
    let deps = &core.inner.deps;
    let ref_price = ref_price
        .filter(|p| !p.is_zero())
        .ok_or("нет опорной цены: стакан монеты не пришёл ни с одной биржи")?;
    let at = At {
        venue: Some(&dir.buy_on.0),
        coin: None,
    };
    let usd = deps
        .settings
        .decimal("money.max_per_trade_usd", at)
        .map_err(|e| e.message)?;
    let cap_qty = usd / ref_price;
    let (qa, qb) = tick_quotes(core, dir, cap_qty).await?;
    // Грубый спред — на максимально вливаемом объёме, не выше потолка (решение 87). fillable_qty ≤
    // запрошенного, поэтому меньшая из глубин обеих сторон уже в пределах потолка. Требовать полного
    // заполнения потолка нельзя: тонкие пулы тогда умирают здесь и не доходят до подбора объёма (66).
    let qty = qa.fillable_qty.min(qb.fillable_qty);
    if qty <= Decimal::ZERO {
        return Err(format!(
            "нет исполнимого объёма: покупка {} / продажа {}",
            qa.fillable_qty, qb.fillable_qty
        ));
    }
    // Потолок не влез целиком — пересчитать обе стороны на реально доступный объём, чтобы цены и спред
    // были согласованы по одному объёму (у сторон разная глубина — берём меньшую).
    let (qa, qb) = if qty < cap_qty {
        tick_quotes(core, dir, qty).await?
    } else {
        (qa, qb)
    };
    let fees = fee_usdt(deps, &qa)? + fee_usdt(deps, &qb)?;
    let notional = qa.avg_price * qty;
    Ok(Prices {
        buy: qa.avg_price,
        sell: qb.avg_price,
        fees_pct: if notional.is_zero() {
            Decimal::ZERO
        } else {
            fees / notional * Decimal::ONE_HUNDRED
        },
        qty,
    })
}

/// Процентные комиссии котировки, которых нет в цене (торговая комиссия, комиссия пула), в USDT.
/// Комиссия в монете переводится по средней цене котировки.
/// Ошибки: не задана котируемая монета площадки; комиссия в другой валюте (её переводит convert.rs).
fn fee_usdt(deps: &Deps, q: &Quote) -> Result<Decimal, String> {
    let quote_ticker = deps
        .settings
        .text(
            "venue.quote_asset",
            At {
                venue: Some(&q.venue.0),
                coin: None,
            },
        )
        .map_err(|e| e.message)?;
    let mut sum = Decimal::ZERO;
    for c in q
        .costs
        .iter()
        .filter(|c| !c.in_price && matches!(c.kind, CostKind::TradeFee | CostKind::PoolFee))
    {
        sum += if c.asset == quote_ticker {
            c.amount
        } else if c.asset == q.asset.ticker {
            c.amount * q.avg_price
        } else {
            return Err(format!(
                "{}: комиссия в {} — в грубом спреде не пересчитывается",
                q.venue.0, c.asset
            ));
        };
    }
    Ok(sum)
}
