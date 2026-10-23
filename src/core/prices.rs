//! Файл: core/prices.rs
//! Что делает: цены направления для грубого спреда (шаг 3): котировки шлюза на объём сравнения
//! на обеих сторонах. Объём сравнения Q = money.max_per_trade_usd площадки A / опорная цена,
//! опорная цена — лучший ask монеты в стакане любой биржи (market_state).
//! Почему котировка, а не только лучшая цена: у пула DEX в стыке нет «лучшей цены» (нужны decimals
//! токенов — запрос в Issue #8), а котировка адаптера заодно проверяет свежесть данных:
//! устарели — ошибка, сигнала нет («не торгуем вслепую»).
//! Кто вызывает: core/engine.rs на каждый тик монеты.
//! От чего зависит: gateway (реестр, котировка), market_state.rs, settings.
//! Настройки: money.max_per_trade_usd (по площадке A), venue.quote_asset (по площадке).

use rust_decimal::Decimal;

use crate::core::types::{CostKind, Direction, Quote, Side};
use crate::settings::read::At;

use super::deps::Deps;
use super::signal::Prices;

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

/// Цены направления для сигнала.
/// Принимает: зависимости, направление, опорную цену монеты (лучший ask биржи; None — стакана нет).
/// Возвращает: Prices. Ошибки: текст причины (нет опорной цены, не задан лимит сделки, нет котировки,
/// не хватает глубины, комиссия в валюте, которую не перевести без convert.rs).
pub async fn for_direction(
    deps: &Deps,
    dir: &Direction,
    ref_price: Option<Decimal>,
) -> Result<Prices, String> {
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
    let qty = usd / ref_price;
    let (qa, qb) = quotes(deps, dir, qty).await?;
    if qa.fillable_qty < qty || qb.fillable_qty < qty {
        return Err(format!(
            "глубины не хватает на объём {qty}: покупка {} / продажа {}",
            qa.fillable_qty, qb.fillable_qty
        ));
    }
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
