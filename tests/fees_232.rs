//! Файл: tests/fees_232.rs
//! Что проверяет: решение 232 — модели монеты комиссии торговли KuCoin, OKX, CoinEx, Phemex, Poloniex, Kraken, HTX, XT
//! по официальной документации и живым фактам бота; разбор факта ордера — список частей по монетам (образец 219).
//! - Живые факты бота 08.11 (tests/data/fees_232_2026-11-08/live_deals_facts.json, GET /api/deals): KuCoin покупка —
//!   feeCurrency USDT, ставка от суммы (0.003); XT покупка WIN — в монете, 0.002 от количества. План комиссии
//!   (gateway/frozen.rs BookFee) этих бирж даёт ту же монету и ту же сумму.
//! - BookFee::in_received (OKX, CoinEx, Poloniex): покупка — в монете от количества, продажа — в USDT от суммы; монета
//!   комиссии покупки совпадает с примерами документации (Poloniex Trades by Order Id — BUY, feeCurrency LINK; CoinEx Get
//!   Order Status — buy, base_fee ≠ 0, quote_fee 0).
//! - Разбор факта: Poloniex сделки ордера (пример документации) — часть в LINK; HTX matchresults и XT deductServices с
//!   двумя монетами — две части, а не ошибка (раньше — ошибка «ядро принимает одну монету»: сделка останавливалась).
//!   HTX и XT с двумя монетами — строки собраны по полям документации (живых ордеров этих бирж в журнале нет).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::frozen, arb::venues::{poloniex::fills, coinex::order_fee, htx::order_fee,
//! xt::order_state}, arb::core::types.

use std::str::FromStr;

use arb::core::types::{Asset, Fee, OrderBook, PriceLevel, Side, VenueId};
use arb::gateway::frozen::{BookFee, FrozenBook};
use arb::venues::coinex::order_fee::{self as coinex_fee, Fee as CoinexFee};
use arb::venues::htx::order_fee as htx_fee;
use arb::venues::poloniex::fills;
use arb::venues::xt::order_state as xt_state;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn data(file: &str) -> Value {
    let p = format!(
        "{}/tests/data/fees_232_2026-11-08/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Стакан из одного уровня по цене исполнения факта: котировка на filled_qty даёт ту же сумму, что у ордера.
fn frozen(
    venue: &str,
    ticker: &str,
    side: Side,
    price: Decimal,
    qty: Decimal,
    fee: BookFee,
) -> FrozenBook {
    let level = vec![PriceLevel { price, qty }];
    let asset = Asset {
        ticker: ticker.into(),
        bsc_contract: None,
    };
    FrozenBook {
        venue: VenueId(venue.into()),
        asset: asset.clone(),
        side,
        book: OrderBook {
            venue: VenueId(venue.into()),
            asset,
            bids: level.clone(),
            asks: level,
            exchange_time: None,
            received_at: Utc::now(),
        },
        fee,
        taken_at: Utc::now(),
    }
}

/// Живые покупки KuCoin и XT: план комиссии адаптера (монета и база расчёта) даёт факт ордера.
#[test]
fn live_facts_match_fee_plan() {
    let facts = data("live_deals_facts.json");
    let mut seen = (0, 0);
    for f in facts.as_array().unwrap() {
        let (venue, ticker) = (f["venue"].as_str().unwrap(), f["ticker"].as_str().unwrap());
        assert_eq!(f["side"], "Buy");
        let (qty, avg, fee) = (
            d(f["filled_qty"].as_str().unwrap()),
            d(f["avg_price"].as_str().unwrap()),
            d(f["fee"].as_str().unwrap()),
        );
        // Ставка — из факта (fee / база); проверяется, что база и монета модели те же, что у биржи.
        let plan = match venue {
            // venues/kucoin/book.rs: USDT от суммы и у покупки.
            "kucoin" => BookFee {
                rate: d("0.003"),
                per_coin: false,
                asset: "USDT".into(),
                paid_in: None,
            },
            // venues/xt/book.rs: покупка — в монете от количества.
            "xt" => BookFee::in_received(d("0.002"), Side::Buy, ticker, "USDT"),
            other => panic!("биржа {other} не ожидалась"),
        };
        if venue == "kucoin" {
            seen.0 += 1;
        } else {
            seen.1 += 1;
        }
        let q = frozen(venue, ticker, Side::Buy, avg, qty, plan).price(qty);
        let c = &q.costs[0];
        assert_eq!(
            c.asset,
            f["fee_asset"].as_str().unwrap(),
            "{venue} {ticker}"
        );
        assert!(c.paid_in.is_none());
        // Сумма модели против факта — до округления биржи (у KuCoin до 1e-8 USDT).
        assert!(
            (c.amount - fee).abs() <= d("0.00000001"),
            "{venue} {ticker}: {} против {fee}",
            c.amount
        );
    }
    assert!(seen.0 >= 5 && seen.1 == 1);
}

/// OKX, CoinEx, Poloniex — комиссия в получаемой монете: покупка в монете, продажа в USDT; монета покупки — как в
/// примерах документации.
#[test]
fn in_received_plan_and_doc_examples() {
    let buy = BookFee::in_received(d("0.001"), Side::Buy, "LINK", "USDT");
    assert!(buy.per_coin && buy.asset == "LINK" && buy.paid_in.is_none());
    let sell = BookFee::in_received(d("0.001"), Side::Sell, "LINK", "USDT");
    assert!(!sell.per_coin && sell.asset == "USDT");
    // Котировка покупки 10 монет по 3.1: комиссия 0.01 LINK (от количества), продажи — 0.031 USDT (от суммы).
    let qb = frozen("poloniex", "LINK", Side::Buy, d("3.1"), d("10"), buy).price(d("10"));
    assert_eq!(
        (qb.costs[0].amount, qb.costs[0].asset.as_str()),
        (d("0.01"), "LINK")
    );
    let qs = frozen("poloniex", "LINK", Side::Sell, d("3.1"), d("10"), sell).price(d("10"));
    assert_eq!(
        (qs.costs[0].amount, qs.costs[0].asset.as_str()),
        (d("0.031"), "USDT")
    );

    // Poloniex: пример документации Trades by Order Id — BUY LINK_USDT, комиссия в LINK.
    let (qty, fees) = fills::sum(&data("poloniex_order_trades_doc.json")).unwrap();
    assert_eq!(qty, d("1"));
    assert_eq!(fees, Fee::single(d("0.00145"), "LINK"));

    // CoinEx: пример документации Get Order Status — покупка CETUSDT, комиссия в базовой монете.
    let s = data("coinex_order_status_doc_fees.json");
    assert_eq!(s["side"], "buy");
    let f = coinex_fee::from_order(&s, "CET", "USDT", "CET").unwrap();
    assert_eq!(f, CoinexFee::Known(d("11.6582326221"), "CET".into()));
}

/// Poloniex: сделки одного ордера в двух монетах комиссии — две части (по полям документации).
#[test]
fn poloniex_two_coins_is_list() {
    let mut v = data("poloniex_order_trades_doc.json");
    let mut second = v[0].clone();
    second["feeCurrency"] = json!("TRX");
    second["feeAmount"] = json!("0.05");
    v.as_array_mut().unwrap().push(second);
    let (qty, fees) = fills::sum(&v).unwrap();
    assert_eq!(qty, d("2"));
    assert_eq!(fees.len(), 2);
    assert_eq!(
        fees[1],
        Fee {
            asset: "TRX".into(),
            amount: d("0.05"),
            usd: None,
        }
    );
}

/// HTX matchresults: одна сделка списана HT (fee-deduct-currency, filled-points), другая — в монете (filled-fees) —
/// две части. Строка собрана по полям документации «Get the Match Result of an Order».
#[test]
fn htx_two_coins_is_list() {
    let data = json!([
        {"filled-amount": "100", "filled-fees": "0", "fee-currency": "cake",
         "fee-deduct-currency": "ht", "filled-points": "0.0123", "fee-deduct-state": "done"},
        {"filled-amount": "50", "filled-fees": "0.1", "fee-currency": "cake",
         "fee-deduct-currency": "", "filled-points": "0"}
    ]);
    let fees = htx_fee::parse(&data, "CAKE").unwrap();
    assert_eq!(
        fees,
        vec![
            Fee {
                asset: "HT".into(),
                amount: d("0.0123"),
                usd: None,
            },
            Fee {
                asset: "CAKE".into(),
                amount: d("0.1"),
                usd: None,
            }
        ]
    );
    assert!(htx_fee::parse(&Value::Null, "CAKE").unwrap().is_empty());
}

/// XT: deductServices в двух монетах — две части; без deductServices — fee/feeCurrency. По полям документации.
#[test]
fn xt_deduct_two_coins_is_list() {
    let s = json!({"orderId": "1", "fee": "1413.932", "feeCurrency": "win",
        "deductServices": [{"fee": "0.01", "feeCurrency": "xt"}, {"fee": "5", "feeCurrency": "win"}]});
    let fees = xt_state::fee(&s).unwrap();
    assert_eq!(
        fees,
        vec![
            Fee {
                asset: "XT".into(),
                amount: d("0.01"),
                usd: None,
            },
            Fee {
                asset: "WIN".into(),
                amount: d("5"),
                usd: None,
            }
        ]
    );
    let plain =
        json!({"orderId": "1", "fee": "1413.932", "feeCurrency": "win", "deductServices": []});
    assert_eq!(
        xt_state::fee(&plain).unwrap(),
        Fee::single(d("1413.932"), "WIN")
    );
}
