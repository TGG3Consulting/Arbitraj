//! Файл: tests/core_fees_219.rs
//! Что проверяет: решение 219 — комиссия торговли динамическая, в любом числе монет.
//! - Bitget: feeDetail живого ордера XTER 08.11 (BGB не хватило: d в BGB и r в XTER) — две части комиссии, а не
//!   ошибка «одна монета» (раньше — OutcomeUnknown, сделка остановлена, монеты вне учёта).
//! - Gate: fee + gt_fee одного ордера — две части; только gt_fee — GT; point_fee — ошибка (ручная проверка).
//! - Старые записи сделок (одна комиссия fee + fee_asset) читаются в список; новые пишутся списком.
//! - Модель котировки (gateway/frozen.rs FrozenBook — та же формула у живой котировки бирж) на записанном стакане Bybit
//!   CAKEUSDT 04.11: комиссия в USDT (MEXC — факт 08.11 214/214) входит в списание spend_fee по худшей цене; в монете
//!   (BingX, Bybit, Bitget — покупка) и монетой скидки (Gate GT, paid_in) — нет.
//! - Сценарий MEXC 30004: объём подогнан ровно под свободный остаток «количество × худшая цена» (как refit без
//!   комиссии) — с комиссией в USDT списание больше остатка; подгонка по spend_max с комиссией (profit.rs: q = qty ×
//!   бюджет / spend_max) даёт списание с комиссией не больше остатка, предел ордера order_cap — тоже.
//!
//! Ставка taker 0.0005 — из живого ответа MEXC tradeFee («0.000500000000000000», шапка venues/mexc/fees.rs).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bitget::order_fee, arb::venues::gate::order_state, arb::core::types (TradeResult, Fee),
//! arb::gateway::frozen, arb::core::costs_types.

use std::str::FromStr;

use arb::core::costs_types::{order_cap, quote_fee_at_worst};
use arb::core::types::{Asset, Fee, OrderBook, PriceLevel, Side, TradeResult, VenueId};
use arb::gateway::frozen::{BookFee, FrozenBook, deepest};
use arb::venues::bitget::order_fee;
use arb::venues::gate::order_state;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn data(path: &str) -> Value {
    let p = format!("{}/tests/data/{path}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Записанный стакан Bybit CAKEUSDT (GET /v5/market/orderbook, 04.11) — стакан ядра.
fn book() -> OrderBook {
    let v = data("cex_samples_2026_11_04/bybit_orderbook_CAKEUSDT.json");
    let lv = |k: &str| -> Vec<PriceLevel> {
        v["result"][k]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| PriceLevel {
                price: d(l[0].as_str().unwrap()),
                qty: d(l[1].as_str().unwrap()),
            })
            .collect()
    };
    OrderBook {
        venue: VenueId("bybit".into()),
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        bids: lv("b"),
        asks: lv("a"),
        exchange_time: None,
        received_at: Utc::now(),
    }
}

/// Снимок покупки с планом комиссии. Принимает: план. Ошибок нет.
fn frozen(fee: BookFee) -> FrozenBook {
    let b = book();
    FrozenBook {
        venue: b.venue.clone(),
        asset: b.asset.clone(),
        side: Side::Buy,
        book: b,
        fee,
        taken_at: Utc::now(),
    }
}

fn plan(per_coin: bool, asset: &str, paid_in: Option<&str>) -> BookFee {
    BookFee {
        rate: d("0.0005"),
        per_coin,
        asset: asset.into(),
        paid_in: paid_in.map(str::to_string),
    }
}

#[test]
fn bitget_two_coins_live_xter() {
    let v = data("fees_2026-11-08/bitget_xter_fee_detail.json");
    let coin = v["baseCoin"].as_str().unwrap().to_ascii_uppercase();
    let fees = order_fee::parse(v.get("feeDetail"), &coin).unwrap();
    assert_eq!(
        fees,
        vec![
            Fee {
                asset: "BGB".into(),
                amount: d("0.0000197"),
                usd: None,
            },
            Fee {
                asset: "XTER".into(),
                amount: d("5.32956"),
                usd: None,
            },
        ]
    );
}

#[test]
fn gate_fee_parts() {
    let both = json!({"fee": "0.01", "fee_currency": "CAKE", "gt_fee": "0.0003", "point_fee": "0"});
    let f = order_state::fee_list(&both, "1").unwrap();
    assert_eq!(f.len(), 2);
    assert_eq!((f[0].asset.as_str(), f[0].amount), ("CAKE", d("0.01")));
    assert_eq!((f[1].asset.as_str(), f[1].amount), ("GT", d("0.0003")));
    let gt = json!({"fee": "0", "fee_currency": "CAKE", "gt_fee": "0.0003", "point_fee": "0"});
    let f = order_state::fee_list(&gt, "1").unwrap();
    assert_eq!(
        f,
        vec![Fee {
            asset: "GT".into(),
            amount: d("0.0003"),
            usd: None,
        }]
    );
    let pts = json!({"fee": "0", "fee_currency": "CAKE", "gt_fee": "0", "point_fee": "0.1"});
    assert!(order_state::fee_list(&pts, "1").is_err());
}

#[test]
fn old_trade_record_reads_into_list() {
    let req = json!({"venue": "mexc", "asset": {"ticker": "CAKE", "bsc_contract": null}, "side": "Buy",
        "qty": "10", "worst_price": "1.9", "deal_id": "x", "route": null});
    let old = json!({"request": req, "status": "Filled", "filled_qty": "10", "avg_price": "1.89",
        "fee": "0.00945", "fee_asset": "USDT", "venue_ref": "1", "chain": null,
        "finished_at": "2026-11-08T10:00:00Z"});
    let r: TradeResult = serde_json::from_value(old).unwrap();
    assert_eq!(r.fee_in("USDT"), d("0.00945"));
    assert_eq!(r.fees.len(), 1);
    // Новая запись — списком; читается обратно так же.
    let back: TradeResult = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
    assert_eq!(back.fees, r.fees);
    // Старая с нулевой комиссией — пустой список.
    let mut zero = serde_json::to_value(&r).unwrap();
    let o = zero.as_object_mut().unwrap();
    o.remove("fees");
    o.insert("fee".into(), json!("0"));
    o.insert("fee_asset".into(), json!("BNB"));
    let z: TradeResult = serde_json::from_value(zero).unwrap();
    assert!(z.fees.is_empty());
}

#[test]
fn quote_model_spend_fee_by_coin() {
    let qty = d("100");
    let worst = deepest(&book(), Side::Buy, qty).unwrap();
    // MEXC: комиссия в USDT от суммы — входит в списание по худшей цене.
    let q = frozen(plan(false, "USDT", None)).price(qty);
    let fee = quote_fee_at_worst(&q, "USDT", worst);
    assert!(fee > Decimal::ZERO);
    assert_eq!(fee, qty * worst * d("0.0005"));
    // BingX/Bybit/Bitget покупка: в монете — не в USDT, списание без неё.
    let q = frozen(plan(true, "CAKE", None)).price(qty);
    assert_eq!(q.costs[0].asset, "CAKE");
    assert_eq!(quote_fee_at_worst(&q, "USDT", worst), Decimal::ZERO);
    // Gate со скидкой GT: сумма в USDT-эквиваленте, платится GT — не списание USDT.
    let q = frozen(plan(false, "USDT", Some("GT"))).price(qty);
    assert_eq!(q.costs[0].paid_in.as_deref(), Some("GT"));
    assert_eq!(quote_fee_at_worst(&q, "USDT", worst), Decimal::ZERO);
}

#[test]
fn refit_with_fee_avoids_30004() {
    let f = frozen(plan(false, "USDT", None));
    let step = d("0.01");
    let spend = |qty: Decimal| {
        let worst = deepest(&f.book, Side::Buy, qty).unwrap();
        let q = f.price(qty);
        let fee = quote_fee_at_worst(&q, "USDT", worst);
        (qty * worst, fee)
    };
    // Как до решения 219: объём подогнан ровно под остаток «количество × худшая цена».
    let q0 = d("150");
    let (notional0, fee0) = spend(q0);
    let free = notional0;
    assert!(
        notional0 + fee0 > free,
        "MEXC заблокирует сумму + комиссию — 30004"
    );
    // Решение 219: spend_max с комиссией больше остатка → подгонка (profit.rs) — с запасом на комиссию.
    let spend_max0 = notional0 + fee0;
    let q1 = (q0 * free / spend_max0 / step).floor() * step;
    let (notional1, fee1) = spend(q1);
    assert!(notional1 + fee1 <= free);
    // Проверка исполнителя (trade_ops): количество × худшая цена ≤ предела ордера.
    assert!(notional1 <= order_cap(free, notional1 + fee1, fee1));
}
