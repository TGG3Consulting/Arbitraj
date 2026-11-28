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
    assert_ne!(address, alloy_primitives::Address::ZERO);
    let asset = Asset {
        ticker: "USDC".into(),
        chain: Some(ChainId("base".into())),
        contract: Some(address),
    };
    let market = Market {
        evm_chain_id: 8453,
        cex_asset_symbol: None,
        cex_network: "BASEEVM".into(),
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

/// Историческая SQLite-схема и запись намерения до передачи площадке. Исходные цены и сеть взяты
/// из публичного снимка. Здесь проверяется только восстановление записанных реквизитов.
#[tokio::test]
async fn legacy_cash_keys_reopen_without_authorizing_a_different_market() {
    use arb::{
        core::{
            settlement_effect, settlement_path,
            types::{DexAccountId, Direction, TradeRequest},
        },
        gateway::{
            pending::PendingRef,
            settlement_intent::{Entry, Request},
        },
        storage::db::Db,
    };
    let (book, _, market) = recorded(Side::Buy);
    let first = &book.book().unwrap().asks[0];
    let deal = "schema-replay";
    let target = Direction {
        asset: market.asset.clone(),
        buy_on: market.dex.clone(),
        sell_on: market.cex.clone(),
    };
    let wallet = DexAccountId("schema-wallet".into());
    let old = serde_json::json!({"plan":{"deal":deal,"direction":target,"cex":market.cex,
        "wallet":wallet,"leg":{"Fund":{"budget_usdt":first.qty*first.price,"buy_usdc":first.qty,
        "maximum_price":first.price,"required_wallet_usdc":first.qty,"target_qty":first.qty,
        "sell_qty":first.qty,"minimum_target_bid":first.price}}},"conversion":null,"movement":null});
    let path: settlement_path::Path = serde_json::from_value(old.clone()).unwrap();
    let prepared = TradeRequest {
        venue: market.cex.clone(),
        asset: market.asset.clone(),
        side: Side::Buy,
        qty: first.qty,
        worst_price: first.price,
        deal_id: deal.into(),
        route: None,
        exit_position: false,
        remeasure: None,
    };
    let entry = Entry::prepared(
        Request::Order(Box::new(prepared)),
        PendingRef::order("USDC_USDT", "t-schema-replay", first.qty),
    );
    let dir = std::env::temp_dir().join(format!(
        "settlement-schema-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir(&dir).unwrap();
    let filename = dir.join("test.sqlite");
    {
        let db = Db::open(&filename).unwrap();
        let path_id = serde_json::to_string(&(deal, &target)).unwrap();
        let order_id = serde_json::to_string(&("order", deal, &market.cex, Side::Buy)).unwrap();
        let body = serde_json::to_string(&entry).unwrap();
        db.ledger().write(move|c|{
            c.execute("INSERT INTO core_state(kind,id,body,updated_at) VALUES('base_settlement_path',?1,?2,'schema')",rusqlite::params![path_id,old.to_string()])?;
            c.execute("INSERT INTO core_state(kind,id,body,updated_at) VALUES('base_settlement_action',?1,?2,'schema')",rusqlite::params![order_id,body])?;
            Ok(())
        }).await.unwrap();
    }
    {
        let db = Db::open(&filename).unwrap();
        let row = settlement_path::get(&db, &path.plan)
            .await
            .unwrap()
            .unwrap();
        assert!(!row.permits(&path.plan, &market));
        assert_eq!(
            arb::gateway::settlement_archive::deal(&db, deal)
                .await
                .unwrap()
                .len(),
            1
        );
        let mut other = market.clone();
        other.evm_chain_id = 1;
        assert!(!row.permits(&path.plan, &other));
        assert!(
            settlement_effect::order_record(&db, &other, deal, &market.cex, Side::Buy)
                .await
                .unwrap()
                .is_none()
        );
        let mut alias = market.clone();
        alias.cex_network = "ETH".into();
        assert!(!row.permits(&path.plan, &alias));
        let mut reassigned = path.plan.clone();
        std::mem::swap(
            &mut reassigned.direction.buy_on,
            &mut reassigned.direction.sell_on,
        );
        assert!(
            !settlement_path::get(&db, &reassigned)
                .await
                .unwrap()
                .unwrap()
                .permits(&reassigned, &market)
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn direct_return_keeps_legacy_destination_optional_and_rejects_mixed_quotes() {
    assert!(settlement::direct_return("USDT", None).is_ok());
    assert!(
        settlement::direct_return("USDT", Some("USDC"))
            .unwrap_err()
            .contains("dex.settlement_contract")
    );
    assert!(settlement::direct_return("USDC", None).is_err());
}

/// Одна настоящая книга Gate применяется к двум независимым публичным квитанциям одной пары в Base.
/// Рыночная оценка конверсии не служит фактом биржевого ордера.
#[tokio::test]
async fn both_cash_directions_reprice_the_recorded_dex_wallet_in_the_same_gate_book() {
    use arb::venues::bsc_dex::{exec::settlement_swap, send::receipt::Receipt};
    let buy: Value =
        serde_json::from_str(include_str!("data/base_settlement_target_buy.json")).unwrap();
    let swaps: Vec<Value> =
        serde_json::from_str(include_str!("data/hashflow_direct_2026-11-26/swaps.json")).unwrap();
    let sell = swaps.into_iter().find(|r| r["chain_id"] == 8453).unwrap();
    let weth = "0x4200000000000000000000000000000000000006"
        .parse()
        .unwrap();
    for (row, side, expected_cash, expected_target) in [
        (
            buy,
            Side::Buy,
            Decimal::new(23557526, 6),
            Decimal::new(8851006401502124, 18),
        ),
        (
            sell,
            Side::Sell,
            Decimal::new(26602020, 6),
            Decimal::new(1, 2),
        ),
    ] {
        let (book, rules, m) = recorded(side);
        let receipt: Receipt = serde_json::from_value(row["receipt"].clone()).unwrap();
        let wallet = row["from"].as_str().unwrap().parse().unwrap();
        let opposite = if side == Side::Buy {
            Side::Sell
        } else {
            Side::Buy
        };
        let usdc =
            settlement_swap::amount(&receipt, wallet, m.asset.contract.unwrap(), side, 6).unwrap();
        let target = settlement_swap::amount(&receipt, wallet, weth, opposite, 18).unwrap();
        assert_eq!((usdc, target), (expected_cash, expected_target));
        assert!(
            settlement_swap::amount(&receipt, wallet, m.asset.contract.unwrap(), opposite, 6)
                .is_none()
        );
        let conversion = settlement::conversion(&book, &rules, &m, side, usdc)
            .await
            .unwrap();
        // Биржевая точность и комиссионная валюта применяются к фактическому объёму.
        assert!(conversion.order_amount <= usdc);
        let iso = if side == Side::Buy {
            conversion.limit_quote
        } else {
            conversion.net_quote
        };
        assert_ne!(iso, usdc);
        // Обе стороны индикативного сигнала уже имеют одну валюту. Даже при неизменной цене комиссия
        // даёт положительный отдельный порог; он измерен по той же книге.
        let rate = conversion.fee_quote + conversion.fee_asset * conversion.limit_price;
        let (entry, exit, threshold) =
            arb::core::settlement_model::signal_amounts(target, iso, iso, rate).unwrap();
        assert_eq!(entry, exit);
        assert!(threshold > Decimal::ZERO);
        assert_eq!(entry, iso / target);
    }
}
