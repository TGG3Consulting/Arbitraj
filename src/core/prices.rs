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
//! Решение 241: котировка стороны DEX направления в открытом окне — точкой цены в окно (window_dex.rs, только память).
//! Решение 260: задан signal.probe_usd — котировки на объём проверки сигнала min(probe, потолок) / опорная цена, не ниже
//! минимумов площадок из памяти (signal_probe.rs) вместо потолка; не задан — побайтно прежний расчёт.
//! Решение 317 (страховка): в Prices — время котировки каждой стороны (Quote.at; у биржи — кадр стакана адаптера).
//! Решение 348: котировки сигнала — с назначением «сигнал» (gateway/quote_purpose.rs, только замер).
//! Решение 352: при signal.book_same_skip = да сторона DEX — из памяти по отпечатку входов (signal_dex.rs).
//! Кто вызывает: core/evaluate.rs на каждый тик монеты.
//! От чего зависит: gateway (реестр, котировка), market_state.rs, quote_cache.rs, signal_probe.rs, settings.
//! Настройки: money.max_per_trade_usd (по площадке A; решение 242 — площадка + тип направления, venue_dir.rs), venue.quote_asset (по площадке), signal.dex_quote_cache_ms,
//! signal.probe_usd (решение 260, та же область, что у потолка).

use rust_decimal::Decimal;

use crate::core::types::{CostKind, Direction, Quote, Side, VenueId};
use crate::settings::read::At;

use super::deps::Deps;
use super::engine::{Core, lk, rd};
use super::settlement_unit::{self, UnitRatio};
use super::signal::Prices;
use crate::gateway::contract::VenueKind;
use crate::gateway::quote_purpose::Purpose::Signal;

/// Котировки обеих сторон направления на объём qty.
/// Принимает: зависимости, направление, объём. Возвращает: (покупка на A, продажа на B).
/// Ошибки: текст причины (площадка не подключена, нет данных, данные устарели).
pub async fn quotes(core: &Core, dir: &Direction, qty: Decimal) -> Result<(Quote, Quote), String> {
    // Решение 132: DEX — кошелёк строки маппинга направления.
    let deps = &core.inner.deps;
    let a = super::dex_account::venue_dir(deps, dir, &dir.buy_on).await?;
    let b = super::dex_account::venue_dir(deps, dir, &dir.sell_on).await?;
    // Решение 211: каждой площадке — её тикер монеты; ответы — обратно в монету ядра.
    let vt = |v: &VenueId| super::venue_ticker::on_venue(core, &dir.asset, v);
    let (on_a, on_b) = (vt(&dir.buy_on), vt(&dir.sell_on));
    let sell_qty = settlement_unit::ratio(core, dir)?.to_sell(qty)?;
    let (qa, qb) = tokio::join!(
        a.quote(&on_a, Side::Buy, qty),
        b.quote(&on_b, Side::Sell, sell_qty)
    );
    let qa = qa.map_err(|e| format!("{}: {}", dir.buy_on.0, e.message))?;
    let qb = qb.map_err(|e| format!("{}: {}", dir.sell_on.0, e.message))?;
    let back = |v: &VenueId, q| super::venue_ticker::quote_back(core, &dir.asset, v, q);
    Ok((back(&dir.buy_on, qa), back(&dir.sell_on, qb)))
}

/// Котировки обеих сторон на тике: сторона DEX — из памяти, если годится (решение 109). Ошибки: как у quotes.
async fn tick_quotes(
    core: &Core,
    dir: &Direction,
    qty: Decimal,
    units: UnitRatio,
) -> Result<(Quote, Quote), String> {
    let sell_qty = units.to_sell(qty)?;
    let (qa, qb) = tokio::join!(
        side_quote(core, dir, Side::Buy, qty),
        side_quote(core, dir, Side::Sell, sell_qty)
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
    // Решение 352: signal.book_same_skip = да — котировка DEX из памяти по отпечатку входов (signal_dex.rs); у площадки
    // без эпохи (KyberSwap, голова не свежая) — прежний путь ниже с signal.dex_quote_cache_ms (ревью t160 Б5).
    if let (true, Some(token)) = (dex, dir.asset.contract)
        && super::signal_dex::on(core)
        && let Some(q) = super::signal_dex::quote(core, dir, venue, token, side, qty).await
    {
        return q;
    }
    let ttl = deps
        .settings
        .integer("signal.dex_quote_cache_ms", At::default())
        .ok()
        .filter(|ms| *ms > 0)
        .map(std::time::Duration::from_millis);
    let cached = match (dex, ttl, dir.asset.contract) {
        (true, Some(ttl), Some(token)) => {
            let key = (venue.clone(), token, side == Side::Buy);
            let cache = lk(&core.inner.quote_cache);
            let generation = cache.generation(venue, token);
            match cache.get(&key, qty, generation, ttl) {
                Some(q) => return Ok(q),
                None => Some((key, generation, ttl)),
            }
        }
        _ => None,
    };
    // Решение 132: DEX — кошелёк строки маппинга направления; нет строки — «нет кошелька DEX в маппинге».
    let a = super::dex_account::venue_dir(deps, dir, venue).await?;
    // Решение 211: площадке — её тикер монеты; ответ — обратно в монету ядра.
    let on = super::venue_ticker::on_venue(core, &dir.asset, venue);
    let q = a
        .quote(&on, side, qty)
        .await
        .map_err(|e| format!("{}: {}", venue.0, e.message))?;
    let q = super::venue_ticker::quote_back(core, &dir.asset, venue, q);
    if let Some((key, generation, ttl)) = cached {
        lk(&core.inner.quote_cache).put(key, qty, q.clone(), generation, ttl);
    }
    Ok(q)
}

/// Цены направления для сигнала.
/// Принимает зависимости, направление и опорную цену за торговую единицу площадки покупки A.
/// Возвращает: Prices на максимально вливаемом объёме (≤ потолка; решение 260 — ≤ объёма проверки signal.probe_usd).
/// Ошибки: текст причины (нет опорной цены, не задан лимит сделки, signal.probe_usd в неверном виде, нет котировки,
/// нет исполнимого объёма ни на сколько — глубина 0, комиссия в валюте, которую не перевести без convert.rs).
pub async fn for_direction(
    core: &Core,
    dir: &Direction,
    ref_price: Option<Decimal>,
) -> Result<Prices, String> {
    let units = settlement_unit::ratio(core, dir)?;
    let ref_price = ref_price
        .filter(|p| *p > Decimal::ZERO)
        .ok_or("нет опорной цены: стакан монеты не пришёл ни с одной биржи")?;
    compare(core, dir, ref_price, units).await
}

/// Книга, как и её цена, принадлежит площадке. Берём A или B, затем проверенный стык A с третьей биржей.
pub(super) async fn for_books(
    core: &Core,
    dir: &Direction,
    books: &[(VenueId, Decimal)],
) -> Result<Prices, String> {
    let units = settlement_unit::ratio(core, dir)?;
    let price_at = |v: &VenueId| {
        books
            .iter()
            .find(|(id, p)| id == v && *p > Decimal::ZERO)
            .map(|(_, p)| *p)
    };
    let px = if let Some(p) = price_at(&dir.buy_on) {
        p
    } else if let Some(p) = price_at(&dir.sell_on) {
        units.to_sell(p)?
    } else {
        books
            .iter()
            .find_map(|(venue, p)| {
                let reference = Direction {
                    asset: dir.asset.clone(),
                    buy_on: dir.buy_on.clone(),
                    sell_on: venue.clone(),
                };
                let unit = settlement_unit::ratio(core, &reference).ok()?;
                unit.to_sell(*p).ok().filter(|p| *p > Decimal::ZERO)
            })
            .ok_or("нет опорной цены в подтверждённых единицах покупки")?
    };
    compare(core, dir, px, units).await
}

/// Объёмы и комиссия каждой котировки остаются нативными вплоть до сравнения на стороне A.
async fn compare(
    core: &Core,
    dir: &Direction,
    ref_price: Decimal,
    units: UnitRatio,
) -> Result<Prices, String> {
    let deps = &core.inner.deps;
    if ref_price <= Decimal::ZERO {
        return Err("опорная цена в единицах покупки округлена до нуля".into());
    }
    let at = At {
        venue: Some(&dir.buy_on.0),
        coin: None,
    };
    // Решение 242: потолок — площадки и типа направления (venue_dir.rs), иначе площадка A, иначе вся система.
    let vd = super::venue_dir::of(core, dir);
    let usd = deps
        .settings
        .decimal_dir("money.max_per_trade_usd", vd, at)
        .map_err(|e| e.message)?;
    // Решение 260: объём проверки сигнала (signal.probe_usd, та же область); не задан — потолок, как раньше.
    let want = match super::signal_probe::probe_usd(&deps.settings, vd, at)? {
        None => usd / ref_price,
        Some(p) => {
            let floor = super::signal_probe::floor_of(core, dir, ref_price, units)?;
            super::signal_probe::qty(usd, Some(p), ref_price, floor)
        }
    };
    // Решение 241: поколение пулов монеты — до котировки (событие пула во время котировки не потеряется).
    let generation = super::window_dex::generation(core, dir);
    // Решение 348: котировки сигнала помечены назначением (замер «кто вызвал»; на расчёт не влияет).
    let (qa, qb) =
        crate::gateway::quote_purpose::scope(Signal, tick_quotes(core, dir, want, units)).await?;
    // Грубый спред — на максимально вливаемом объёме, не выше потолка (решение 87; решение 260 — не выше объёма
    // проверки). fillable_qty ≤ запрошенного, поэтому меньшая из глубин обеих сторон уже в его пределах. Требовать
    // полного заполнения потолка нельзя: тонкие пулы тогда умирают здесь и не доходят до подбора объёма (66).
    let qty = common_depth(qa.fillable_qty, qb.fillable_qty, units)?;
    if qty <= Decimal::ZERO {
        return Err(format!(
            "нет исполнимого объёма: покупка {} / продажа {}",
            qa.fillable_qty, qb.fillable_qty
        ));
    }
    // Запрошенный объём не влез целиком — пересчитать обе стороны на реально доступный объём, чтобы цены и спред
    // были согласованы по одному объёму (у сторон разная глубина — берём меньшую).
    let (qa, qb) = if qty < want {
        crate::gateway::quote_purpose::scope(Signal, tick_quotes(core, dir, qty, units)).await?
    } else {
        (qa, qb)
    };
    super::window_dex::note(core, dir, &qa, &qb, generation);
    let fees = fee_usdt(deps, &qa)? + fee_usdt(deps, &qb)?;
    quoted_prices(&qa, &qb, qty, fees, units)
}

/// Сравнивает нативные глубины. Если B уже покрывает A, не округляем объём дважды при A → B → A.
fn common_depth(a: Decimal, b: Decimal, units: UnitRatio) -> Result<Decimal, String> {
    if b >= units.to_sell(a)? {
        Ok(a)
    } else {
        Ok(a.min(units.to_buy(b)?))
    }
}

/// Цена за единицу A для сравнения. Исходную котировку B сохраняем для её стакана и ряда σ.
fn quoted_prices(
    qa: &Quote,
    qb: &Quote,
    qty: Decimal,
    fees: Decimal,
    units: UnitRatio,
) -> Result<Prices, String> {
    let notional = qa.avg_price * qty;
    Ok(Prices {
        buy: qa.avg_price,
        sell: units.to_sell(qb.avg_price)?,
        venue_sale: (!units.is_identity()).then_some(super::signal_types::VenueSale {
            price: qb.avg_price,
            qty: qb.fillable_qty,
        }),
        fees_pct: if notional.is_zero() {
            Decimal::ZERO
        } else {
            fees / notional * Decimal::ONE_HUNDRED
        },
        qty,
        buy_at: qa.at,
        sell_at: qb.at,
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

#[cfg(test)]
#[path = "prices_unit_tests.rs"]
mod unit_tests;
