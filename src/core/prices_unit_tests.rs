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

#[test]
fn native_protocol_payment_is_priced_once_in_signal_plan_and_settlement() {
    use crate::core::{
        convert, fills,
        types::{Cost, CostKind, Fee},
    };
    use std::str::FromStr;
    let row: Market = serde_json::from_str(include_str!(
        "../../tests/data/bsc_units_2026_11_24/signal_quotes.json"
    ))
    .unwrap();
    let mut quote = row.sell_quote;
    // Из архивного lpInfo() Ethereum, отдельно от условных курса ETH и сетевого газа.
    let fee = Decimal::from_str("0.00018").unwrap();
    let gas = Decimal::from_str("0.00005").unwrap();
    let rate = Decimal::from(3000);
    let rates = convert::Rates {
        quote: vec!["USDT".into()],
        coin: quote.asset.ticker.clone(),
        coin_price: quote.avg_price,
        prices: [("ETH".into(), rate)].into_iter().collect(),
    };
    let flat = Cost::new(CostKind::ProtocolFee, fee, "ETH", false);
    let network = Cost::new(CostKind::Gas, gas, "ETH", false);
    quote.costs = vec![flat.clone(), network];
    quote.costs.push(Cost {
        in_price: true,
        ..flat
    });
    let signal: Decimal = protocol_payments(&quote)
        .map(|c| convert::to_usdt(c, &rates).unwrap())
        .sum();
    let plan: Decimal = quote
        .costs
        .iter()
        .filter(|c| !c.in_price)
        .map(|c| convert::to_usdt(c, &rates).unwrap())
        .sum();
    assert_eq!(signal, fee * rate);
    assert_eq!(plan, signal + gas * rate);
    let mut actual = Fee::single(gas, "ETH");
    Fee::add_protocol(&mut actual, "ETH", fee);
    let actual: Vec<Fee> = serde_json::from_value(serde_json::to_value(actual).unwrap()).unwrap();
    assert_eq!(actual.len(), 2);
    let kinds: Vec<_> = actual.iter().map(|f| fills::fee_kind(true, f)).collect();
    assert_eq!(kinds, [CostKind::Gas, CostKind::ProtocolFee]);
    let spent: Decimal = actual
        .iter()
        .zip(kinds)
        .map(|(f, k)| convert::to_usdt(&Cost::new(k, f.amount, &f.asset, false), &rates).unwrap())
        .sum();
    assert_eq!(plan, spent);
    assert!(!convert::in_coin(&quote.costs[0], "ETH"));
}
