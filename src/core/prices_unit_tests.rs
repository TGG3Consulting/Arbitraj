//! Расчёт SNDKB на сохранённых ценах из плана 20261124142525658SNDKB.

use super::*;
use crate::core::signal::{Params, Signals, State};
use serde::Deserialize;

#[derive(Deserialize)]
struct Market {
    buy_quote: Quote,
    sell_quote: Quote,
}

#[test]
fn recorded_prices_count_in_both_directions_without_changing_native_depth() {
    let row: Market = serde_json::from_str(include_str!(
        "../../tests/data/bsc_units_2026_11_24/signal_quotes.json"
    ))
    .unwrap();
    let (cex, dex) = (&row.buy_quote, &row.sell_quote);
    let shares = Decimal::from(8u32);
    let to_chain = UnitRatio {
        buy_per_raw: shares,
        sell_per_raw: Decimal::ONE,
        raw_decimals: Some(18),
    };
    let to_exchange = UnitRatio {
        buy_per_raw: Decimal::ONE,
        sell_per_raw: shares,
        raw_decimals: Some(18),
    };

    let state = |buy: &Quote, sell: &Quote, units: UnitRatio| {
        let qty = common_depth(buy.fillable_qty, sell.fillable_qty, units).unwrap();
        let p = quoted_prices(buy, sell, qty, Decimal::ZERO, units).unwrap();
        assert_eq!(p.venue_sale.as_ref().unwrap().price, sell.avg_price);
        let dir = Direction {
            asset: buy.asset.clone(),
            buy_on: buy.venue.clone(),
            sell_on: sell.venue.clone(),
        };
        let params = Ok(Params {
            hold_ms: 0,
            hold_updates: 1,
            anomaly_pct: Decimal::ONE_HUNDRED,
            skip_on_pool: false,
            ripen: false,
        });
        Signals::default()
            .update(&dir, Ok(p), &params, buy.at)
            .state
    };
    // Фактические стороны: 651,55 USDT за сетевой токен меньше 8 × 83,27 USDT.
    assert_eq!(state(cex, dex, to_chain), State::Below);
    // Положительный контроль математики: переставляем те же записанные цены и единицы местами.
    // Это проверка прохождения сигнала, а не историческая котировка обратного направления.
    assert_eq!(state(dex, cex, to_exchange), State::Candidate);
    // При ограничивающей глубине CEX: 3,240427 биржевых единиц, а не столько же сетевых.
    assert_eq!(
        common_depth(dex.fillable_qty, cex.fillable_qty, to_exchange).unwrap(),
        cex.fillable_qty / shares
    );
}
