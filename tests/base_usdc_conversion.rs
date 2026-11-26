//! Регрессия Base USDC: снимок Gate /spot/order_book, /spot/currency_pairs/USDC_USDT и /spot/currencies/USDC.
//! Публичная комиссия здесь обозначает модель; боевой путь получает план комиссии аккаунта от адаптера.
use arb::core::{
    settlement,
    settlement_market::Market,
    types::{Asset, ChainId, OrderBook, PriceLevel, Side, VenueId},
};
use arb::gateway::{
    events::SpotPairState,
    frozen::{BookFee, FrozenBook, QuoteSnapshot},
};
use chrono::DateTime;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::str::FromStr;
use std::sync::Arc;

/// Декодирует зафиксированный снимок Gate. Время и уровни берутся из записи ответа.
fn recorded(side: Side) -> (QuoteSnapshot, SpotPairState, Market) {
    let v: Value = serde_json::from_str(include_str!("data/base_usdc_gate_market.json")).unwrap();
    let now = DateTime::from_timestamp_millis(v["book"]["current"].as_i64().unwrap()).unwrap();
    let parsed = arb::venues::gate::symbols_parse::parse_list(
        &json!([v["pair"]]),
        "USDT",
        &Default::default(),
        now.timestamp(),
    )
    .unwrap();
    let p = &parsed["USDC"];
    let pair = SpotPairState {
        ticker: "USDC".into(),
        status: p.status,
        online: p.online(),
        api_buy: p.api_buy,
        api_sell: p.api_sell,
        step_size: p.step_size,
        min_notional: p.min_notional,
        max_notional: p.max_notional,
        min_qty: p.min_qty,
        max_qty: p.max_qty,
        order_block: None,
        price_band: None,
    };
    let venue = VenueId("gate".into());
    let address = v["network"]["chains"][0]["addr"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(arb::settings::evm_profiles::LEGACY_CASH_TOKEN, address);
    let asset = Asset {
        ticker: "USDC".into(),
        chain: Some(ChainId("base".into())),
        contract: Some(address),
    };
    let market = Market {
        evm_chain_id:8453,cex_network:"BASEEVM".into(),
        dex: VenueId("base_dex".into()),
        cex: venue.clone(),
        chain: ChainId("base".into()),
        asset: asset.clone(),
        quote: "USDT".into(),
    };
    let levels = |s: &str| {
        v["book"][s]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| PriceLevel {
                price: Decimal::from_str(l[0].as_str().unwrap()).unwrap(),
                qty: Decimal::from_str(l[1].as_str().unwrap()).unwrap(),
            })
            .collect()
    };
    let book = OrderBook {
        venue: venue.clone(),
        asset: asset.clone(),
        bids: levels("bids"),
        asks: levels("asks"),
        exchange_time: Some(now),
        received_at: now,
    };
    let rate =
        Decimal::from_str(v["pair"]["fee"].as_str().unwrap()).unwrap() / Decimal::ONE_HUNDRED;
    (
        Arc::new(FrozenBook {
            venue: venue.clone(),
            asset,
            side,
            book,
            fee: BookFee::in_received(rate, side, "USDC", "USDT"),
            taken_at: now,
        }),
        pair,
        market,
    )
}

#[tokio::test]
async fn buy_and_sell_use_different_liquidity_and_round_down() {
    let qty = Decimal::from_str("200.009").unwrap();
    let (buy_book, rules, venue) = recorded(Side::Buy);
    let buy = settlement::conversion(&buy_book, &rules, &venue, Side::Buy, qty)
        .await
        .unwrap();
    assert_eq!(buy.order_amount, Decimal::from(200));
    assert_eq!(buy.net_asset, Decimal::from_str("199.8").unwrap());
    // Запрос пересекает первую ask на 111.26 USDC.
    assert!(buy.limit_price > buy_book.book().unwrap().asks[0].price);
    assert!(buy.limit_quote >= buy.net_quote);
    let (sell_book, rules, venue) = recorded(Side::Sell);
    let sell = settlement::conversion(&sell_book, &rules, &venue, Side::Sell, qty)
        .await
        .unwrap();
    assert_eq!(sell.limit_price, sell_book.book().unwrap().bids[0].price);
    assert!(sell.net_quote < buy.net_quote);
    assert!(sell.fee_quote > Decimal::ZERO);
    assert!(
        settlement::conversion(&sell_book, &rules, &venue, Side::Sell, Decimal::ONE)
            .await
            .is_err()
    );
    let depth: Decimal = sell_book
        .book()
        .unwrap()
        .bids
        .iter()
        .map(|level| level.qty)
        .sum();
    assert!(
        settlement::conversion(&sell_book, &rules, &venue, Side::Sell, depth + Decimal::ONE)
            .await
            .is_err()
    );
    assert!(settlement::require_same("USDC", "USDT").is_err());
}
