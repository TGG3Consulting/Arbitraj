//! Файл: tests/size_common/calc.rs
//! Что делает: расчёт ядра (core::costs::Calc) на записанных данных CAKE — для проверок решения 74 (сухой режим,
//! эпизоды): покупка CAKE по записанным asks BingX (bingx::book::walk), продажа в записанный пул CAKE/USDT V2
//! (адаптер DEX quote/pool_quote::swap). Затрат сверх цены в записях нет (ставки комиссии, газ — см. mod.rs), поэтому
//! список затрат пуст, прибыль — выручка минус вложение.
//! Кто вызывает: tests/core_dry_mode.rs, tests/core_episodes.rs.
//! От чего зависит: tests/size_common/mod.rs.

use arb::core::costs::Calc;
use arb::core::types::{Asset, Direction, Quote, Side, VenueId};
use arb::venues::bingx::book::walk;
use arb::venues::bsc_dex::num::{to_decimal, to_units};
use arb::venues::bsc_dex::quote::pool_quote::swap;
use chrono::Utc;
use rust_decimal::Decimal;

use super::{CAKE, Rec};

/// Направление bingx → bsc_dex по CAKE с адресом из записи.
pub fn direction() -> Direction {
    Direction {
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE),
        },
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
    }
}

/// Котировка стороны.
fn quote(venue: &str, side: Side, qty: Decimal, avg: Decimal) -> Quote {
    Quote {
        venue: VenueId(venue.into()),
        asset: direction().asset,
        side,
        qty,
        fillable_qty: qty,
        avg_price: avg,
        costs: vec![],
        route: None,
        signal_route: None,
        at: Utc::now(),
    }
}

/// Расчёт на qty монет по записям. Принимает: записи, объём (хватает глубины asks).
pub fn calc(rec: &Rec, qty: Decimal) -> Calc {
    let (filled, spent) = walk(&rec.asks, qty);
    assert_eq!(filled, qty, "глубины записи хватает");
    let out = swap(&rec.ctx, &rec.pool, CAKE, true, to_units(qty, 18).unwrap()).unwrap();
    let proceeds = to_decimal(out.amount_out, 18).unwrap();
    let (buy_avg, sell_avg) = (spent / qty, proceeds / qty);
    let profit = proceeds - spent;
    Calc {
        direction: direction(),
        qty,
        arrive: qty,
        sell_qty: qty,
        buy_avg,
        sell_avg,
        invested: spent,
        proceeds,
        costs: vec![],
        costs_usdt: Decimal::ZERO,
        profit_usd: profit,
        profit_pct: profit / spent * Decimal::ONE_HUNDRED,
        transit_s: Decimal::ZERO,
        sigma: Decimal::ZERO,
        buy_quote: quote("bingx", Side::Buy, qty, buy_avg),
        sell_quote: quote("bsc_dex", Side::Sell, qty, sell_avg),
        at: Utc::now(),
        spend_max: spent,
        size: None,
    }
}
