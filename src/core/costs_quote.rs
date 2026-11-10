//! Файл: core/costs_quote.rs
//! Что делает: котировка стороны сделки для полного расчёта (costs.rs) и пределы ордера биржи с видом отказа для
//! подбора объёма (решение 66):
//! - глубины не хватает или у DEX нет пути на такой объём (NoRoute: объём ≥ резерва пула) — объём слишком большой;
//! - пределы спот-пары биржи — количество (minQty / maxQty) и сумма (minNotional / maxNotional) по ХУДШЕЙ цене
//!   ордера — той же, что ставит исполнение (trade_ops::worst_price, лимитная цена) и проверяет адаптер BingX
//!   (venues/bingx/trade.rs: qty × price); соблюдаются оба предела, строже — тот, что сработал;
//! - предел 0 у максимума (maxQty, maxNotional) — «нет данных», расчёт невозможен (в записях не встречался, вопрос
//!   Тиграну, решение 66); 0 у минимума — ограничения нет;
//! - количество — вниз до шага количества пары (stepSize).
//!
//! Сбой шлюза — расчёт невозможен. Решение 217: со снимком рынка момента решения (core/frozen.rs) котировка и худшая
//! цена — по снимку стороны, не по живым данным (расчёт «без потолка» по сделке).
//! Решение 250: fillable — исполнимое количество через контракт quote_fills (проверка доли глубины h), enough_qty.
//! Кто вызывает: core/costs.rs (enough, enough_qty, floor_step, order_limits), core/costs_source.rs (fetch, fillable, worst — источник Live),
//! core/uncapped_replay.rs (пересчёт по окнам, решение 218: enough, floor_step).
//! От чего зависит: core/types, core/size.rs (Fail), deps.rs (шлюз), trade_ops.rs, frozen.rs, gateway/events.rs (SpotPairState),
//! gateway/frozen.rs (deepest).
//! Настройки: через trade_ops.rs — exec.dex_slippage_pct (худшая цена DEX).

use rust_decimal::Decimal;

use crate::core::types::{Asset, Direction, Quote, Side, VenueId};
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::gateway::events::SpotPairState;

use super::engine::Core;
use super::frozen::Frozen;
use super::size::Fail;

/// Котировка стороны (источник Live, costs_source.rs; глубину проверяет enough). Принимает: ядро, направление, сторону
/// (true — продажа на B), количество, снимок рынка момента решения (решение 217; None — живая котировка площадки).
/// Ошибки: TooLarge — NoRoute у площадки; Stop — прочие сбои шлюза.
pub async fn fetch(
    core: &Core,
    d: &Direction,
    sell: bool,
    qty: Decimal,
    src: Option<&Frozen>,
) -> Result<Quote, Fail> {
    let (venue, side) = if sell {
        (&d.sell_on, Side::Sell)
    } else {
        (&d.buy_on, Side::Buy)
    };
    let q = match src {
        // Решение 217: по снимку стороны (у биржи — её стакан, у DEX — пулы на момент решения).
        Some(f) => f.side(sell).quote(qty).await,
        None => {
            // Решение 132: DEX — кошелёк строки маппинга направления (его разрешения и исполнитель); нет строки — отказ.
            let v = super::dex_account::venue_dir(&core.inner.deps, d, venue)
                .await
                .map_err(Fail::Stop)?;
            // Решение 211: площадке — её тикер монеты; ответ — обратно в монету ядра.
            let on = super::venue_ticker::on_venue(core, &d.asset, venue);
            v.quote(&on, side, qty).await
        }
    }
    .map_err(|e| classify(&venue.0, &e))?;
    Ok(super::venue_ticker::quote_back(core, &d.asset, venue, q))
}

/// Проверка глубины котировки (общая для всех источников, costs.rs). Принимает: площадку (текст), котировку,
/// запрошенное количество. Возвращает: ту же котировку. Ошибки: TooLarge — глубины хватает не на всё количество.
pub fn enough(venue: &str, q: Quote, qty: Decimal) -> Result<Quote, Fail> {
    enough_qty(venue, q.fillable_qty, qty)?;
    Ok(q)
}

/// Решение 250: та же проверка глубины по исполнимому количеству (проверка доли глубины h — costs.rs). Принимает:
/// площадку (текст), исполнимое количество, запрошенное. Ошибки: TooLarge — тот же текст, что у enough.
pub fn enough_qty(venue: &str, fillable: Decimal, qty: Decimal) -> Result<(), Fail> {
    if fillable < qty {
        return Err(Fail::TooLarge(format!(
            "{venue}: глубины хватает на {fillable} из {qty}"
        )));
    }
    Ok(())
}

/// Решение 250: исполнимое количество стороны без цены (источник Live без снимка; проверка доли глубины h) —
/// контракт quote_fills площадки (по умолчанию — fillable_qty полной котировки; bsc_dex — quote/fills.rs). Тот же
/// кошелёк маппинга и тикер площадки, что у fetch. Принимает: ядро, направление, сторону, количество.
/// Возвращает: исполнимое количество. Ошибки: как у fetch (TooLarge — NoRoute, Stop — прочие сбои).
pub async fn fillable(
    core: &Core,
    d: &Direction,
    sell: bool,
    qty: Decimal,
) -> Result<Decimal, Fail> {
    let (venue, side) = if sell {
        (&d.sell_on, Side::Sell)
    } else {
        (&d.buy_on, Side::Buy)
    };
    let v = super::dex_account::venue_dir(&core.inner.deps, d, venue)
        .await
        .map_err(Fail::Stop)?;
    let on = super::venue_ticker::on_venue(core, &d.asset, venue);
    v.quote_fills(&on, side, qty)
        .await
        .map_err(|e| classify(&venue.0, &e))
}

/// Вид отказа котировки площадки. Принимает: площадку (для текста), ошибку шлюза. Возвращает: TooLarge — NoRoute
/// (у DEX: объём не меньше резерва пула, «расчёт обмена невозможен» — меньший объём может пройти), иначе Stop.
pub fn classify(venue: &str, e: &GatewayError) -> Fail {
    let t = format!("{venue}: {}", e.message);
    if e.code == ErrorCode::NoRoute {
        Fail::TooLarge(t)
    } else {
        Fail::Stop(t)
    }
}

/// Худшая цена ордера, как её поставит исполнение (trade_ops::worst_price). Решение 217: со снимком у биржи — самый
/// глубокий нужный уровень стакана снимка (у DEX — та же формула допуска). Принимает: ядро, площадку, монету, сторону,
/// количество, среднюю цену, снимок (None — живой стакан ядра). Ошибки: Stop — нет стакана, глубины или допуска.
pub fn worst(
    core: &Core,
    v: &VenueId,
    a: &Asset,
    side: Side,
    qty: Decimal,
    avg: Decimal,
    src: Option<&Frozen>,
) -> Result<Decimal, Fail> {
    if let Some(book) = src.and_then(|f| f.book(side == Side::Sell)) {
        return crate::gateway::frozen::deepest(book, side, qty).ok_or_else(|| {
            Fail::Stop(format!(
                "{}: глубины стакана снимка не хватает на {qty}",
                v.0
            ))
        });
    }
    super::trade_ops::worst_price(core, v, a, side, qty, avg).map_err(Fail::Stop)
}

/// Количество, округлённое вниз до шага пары биржи (у DEX пары нет — без изменений; шаг DEX — 10^-decimals
/// монеты — адаптер применяет сам, одинаково в котировке и в обмене). Ошибок нет.
pub fn floor_step(qty: Decimal, pair: Option<&SpotPairState>) -> Decimal {
    match pair {
        Some(p) if p.step_size > Decimal::ZERO => (qty / p.step_size).floor() * p.step_size,
        _ => qty,
    }
}

/// Пределы ордера биржи. Принимает: площадку (для текста), пару (None — DEX, пределов нет), количество, худшую цену.
/// Ошибки: TooSmall — меньше minQty или minNotional; TooLarge — больше maxQty или maxNotional; Stop — максимум 0.
pub fn order_limits(
    venue: &str,
    pair: Option<&SpotPairState>,
    qty: Decimal,
    price: Decimal,
) -> Result<(), Fail> {
    let Some(p) = pair else { return Ok(()) };
    let sum = qty * price;
    let checks = [
        ("количество", qty, p.min_qty, p.max_qty, "minQty", "maxQty"),
        (
            "сумма ордера",
            sum,
            p.min_notional,
            p.max_notional,
            "minNotional",
            "maxNotional",
        ),
    ];
    for (what, v, min, max, min_name, max_name) in checks {
        if let Some(min) = min
            && v < min
        {
            return Err(Fail::TooSmall(format!(
                "{venue}: {what} {} меньше {min_name} {min}",
                v.round_dp(8)
            )));
        }
        match max {
            Some(m) if m.is_zero() => {
                return Err(Fail::Stop(format!(
                    "{venue}: {max_name} = 0 — предел неизвестен, сделки нет"
                )));
            }
            Some(m) if v > m => {
                return Err(Fail::TooLarge(format!(
                    "{venue}: {what} {} больше {max_name} {m}",
                    v.round_dp(8)
                )));
            }
            _ => {}
        }
    }
    Ok(())
}
