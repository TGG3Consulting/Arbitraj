//! Файл: tests/kucoin_real_data.rs
//! Что делает: адаптер KuCoin (решение 137) на ЖИВЫХ публичных ответах 2026-11-04 (tests/data/cex_samples_2026_11_04/
//! kucoin_*, записаны на сервере curl/websockets как есть) и на официальном тестовом векторе подписи SDK:
//! - подпись: Base64(HMAC-SHA256) — вектор test_default_signer.py официального SDK github.com/Kucoin/kucoin-universal-sdk;
//! - справочник пар /api/v2/symbols: CAKE-USDT — шаги 0.0001 / 0.001, minFunds 0.1, baseMinSize 0.01; правила ордера
//!   (order_rules::check_rules — та же функция у ордера и у check_order);
//! - справочник монет /api/v3/currencies и /api/v3/currencies/CAKE: сеть bsc (chainName BEP20), адрес контракта CAKE,
//!   комиссия вывода withdrawalMinFee 0.41 при withdrawFeeRate 0, минимум депозита, точность вывода;
//! - токен bullet-public: адрес сервера, pingInterval 18000, pingTimeout 10000;
//! - кадры WebSocket: welcome, ack, pong, снимок /spotMarket/level2Depth50, изменения /market/level2; снимок REST level2_20.
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::kucoin. Моков нет.

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{Side, TapView, VenueId};
use arb::gateway::tap::{TapData, TapStream};
use arb::venues::kucoin::{bullet, depth, networks_parse, order_rules, sign, symbols, tap_decode};

const DIR: &str = "tests/data/cex_samples_2026_11_04";

fn data(file: &str) -> Value {
    let t = std::fs::read_to_string(format!("{DIR}/{file}")).expect("файл записи");
    let mut v: Value = serde_json::from_str(&t).expect("JSON");
    assert_eq!(v["code"], "200000");
    v.as_object_mut().unwrap().remove("data").unwrap()
}

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

#[test]
fn signature_matches_official_sdk_vector() {
    // sdk/python/kucoin_universal_sdk/internal/infra/test_default_signer.py: sign(b"test_plain", b"test_api_secret").
    let s = sign::hmac_b64(b"test_plain", b"test_api_secret").unwrap();
    assert_eq!(s, "jie/gxS38wLFFaBwl/cAzFbAzlI8MIny96jSXrB+iP4=");
    assert_eq!(
        sign::prehash(1, "get", "/api/v1/accounts?type=trade", ""),
        "1GET/api/v1/accounts?type=trade"
    );
}

#[test]
fn symbols_cake_rules_and_order_rules() {
    let (map, skipped) = symbols::parse_list(&data("kucoin_symbols_v2.json"), "USDT").unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    assert!(map.len() > 500);
    let cake = &map["CAKE"];
    assert_eq!(cake.symbol, "CAKE-USDT");
    assert!(cake.tradable());
    assert_eq!(cake.step_size, d("0.0001"));
    assert_eq!(cake.tick_size, d("0.001"));
    assert_eq!(cake.min_notional, Some(d("0.1")));
    assert_eq!(cake.min_qty, Some(d("0.01")));
    assert_eq!(cake.taker_coef, Decimal::ONE);
    // Покупка: количество вниз до шага, цена вниз до шага; продажа — цена вверх.
    let (q, p) =
        order_rules::check_rules(cake, Side::Buy, d("10.12345"), d("1.8919"), "IOC").unwrap();
    assert_eq!((q, p), (d("10.1234"), d("1.891")));
    let (_, p) = order_rules::check_rules(cake, Side::Sell, d("10"), d("1.8911"), "IOC").unwrap();
    assert_eq!(p, d("1.892"));
    // Сумма меньше minFunds, количество меньше baseMinSize, срок GTC — отказ до отправки.
    assert!(order_rules::check_rules(cake, Side::Buy, d("0.05"), d("1.8"), "IOC").is_err());
    assert!(order_rules::check_rules(cake, Side::Buy, d("0.009"), d("100"), "IOC").is_err());
    assert!(order_rules::check_rules(cake, Side::Buy, d("10"), d("1.8"), "GTC").is_err());
}

#[test]
fn currencies_bsc_network() {
    let venue = VenueId("kucoin".into());
    let all =
        networks_parse::parse(&data("kucoin_currencies_v3.json"), "bsc", &venue, false).unwrap();
    assert!(all.skipped.is_empty(), "{:?}", all.skipped);
    // Одна сеть bsc с ненулевой долей комиссии в записи. Решение 209: комиссия = withdrawalMinFee + доля × сумма
    // (факт экрана вывода KuCoin 07.11: LUNC 1 → 8 000,03 при minFee 8000 и rate 0.03).
    assert_eq!(all.fee_rate.len(), 1, "{:?}", all.fee_rate);
    let (tk, raw) = all.fee_rate[0].split_once('=').unwrap();
    let net = all
        .list
        .iter()
        .find(|n| n.asset.ticker == tk && n.network == "bsc")
        .unwrap();
    assert_eq!(net.withdraw_fee_pct, Some(d(raw.trim_matches('"'))));
    assert!(
        net.withdraw_fee.is_some(),
        "фиксированная часть — withdrawalMinFee"
    );
    let one =
        networks_parse::parse(&data("kucoin_currency_CAKE_v3.json"), "bsc", &venue, false).unwrap();
    assert_eq!(one.list.len(), 1);
    let cake = &one.list[0];
    assert_eq!(cake.network, "bsc");
    let addr: alloy::primitives::Address = "0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82"
        .parse()
        .unwrap();
    assert_eq!(cake.asset.bsc_contract, Some(addr));
    assert_eq!(cake.withdraw_fee, Some(d("0.41")));
    assert_eq!(cake.withdraw_min, Some(d("0.82")));
    assert_eq!(cake.deposit_min, Some(d("0.01")));
    assert_eq!(cake.withdraw_precision, Some(8));
    assert_eq!(cake.deposit_confirmations, Some(60));
    assert!(cake.deposit_enabled && cake.withdraw_enabled);
    let in_all = all
        .list
        .iter()
        .find(|i| i.asset.ticker == "CAKE" && i.network == "bsc")
        .unwrap();
    assert_eq!(in_all.asset.bsc_contract, Some(addr));
}

#[test]
fn bullet_public_structure() {
    let t = bullet::parse(&data("kucoin_bullet_public.json"), "c1").unwrap();
    assert!(t.url.starts_with("wss://ws-api-spot.kucoin.com/?token="));
    assert!(t.url.ends_with("&connectId=c1"));
    assert_eq!(t.ping.as_millis(), 18000);
    assert_eq!(t.ping_timeout.as_millis(), 10000);
}

/// Кадры записи kucoin_ws.jsonl (поле msg — кадр как пришёл).
fn ws_frames() -> Vec<String> {
    let t = std::fs::read_to_string(format!("{DIR}/kucoin_ws.jsonl")).expect("запись WebSocket");
    t.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v.get("msg").and_then(Value::as_str).map(str::to_string))
        .collect()
}

#[test]
fn ws_frames_parse() {
    let (mut welcome, mut acks, mut pong, mut books, mut diffs) = (0, 0, 0, 0, 0);
    for f in ws_frames() {
        match depth::parse(&f) {
            depth::Msg::Welcome => welcome += 1,
            depth::Msg::Ack(_) => acks += 1,
            depth::Msg::Pong => pong += 1,
            depth::Msg::Depth {
                symbol,
                bids,
                asks,
                exchange_time,
            } => {
                assert_eq!(symbol, "CAKE-USDT");
                assert!(!bids.is_empty() && !asks.is_empty() && exchange_time.is_some());
                assert!(bids[0].price < asks[0].price);
                assert!(asks.windows(2).all(|w| w[0].price < w[1].price));
                assert!(bids.windows(2).all(|w| w[0].price > w[1].price));
                let dec = tap_decode::decode(TapStream::Book, &TapData::Text(f.clone())).unwrap();
                assert!(matches!(dec.view, TapView::Book { .. }));
                books += 1;
            }
            depth::Msg::Data { topic, .. } if topic == "/market/level2:CAKE-USDT" => {
                let (from, to) = tap_decode::diff_range(&f).unwrap();
                assert!(from <= to);
                let dec = tap_decode::decode(TapStream::Diff, &TapData::Text(f.clone())).unwrap();
                assert!(matches!(dec.view, TapView::Diff { full: false, .. }));
                diffs += 1;
            }
            depth::Msg::Bad { reason, .. } => panic!("кадр не разобран: {reason}"),
            _ => {}
        }
    }
    assert_eq!((welcome, acks, pong), (1, 3, 1));
    assert_eq!((books, diffs), (11, 18));
}

#[test]
fn rest_snapshot_decode() {
    let t = std::fs::read_to_string(format!("{DIR}/kucoin_orderbook_level2_20_CAKE-USDT.json"))
        .unwrap();
    let dec = tap_decode::decode(TapStream::Snapshot, &TapData::Text(t)).unwrap();
    let TapView::Diff {
        bids,
        asks,
        full,
        from,
        ..
    } = dec.view
    else {
        panic!("не снимок")
    };
    assert!(full && from == Some(783409027));
    assert_eq!((bids.len(), asks.len()), (20, 20));
}

#[test]
#[ignore = "нет живой записи сделок /market/match KuCoin — tests/README.md"]
fn recorded_match_decode() {
    let t = std::fs::read_to_string(format!("{DIR}/kucoin_match.jsonl")).expect("запись сделок");
    for l in t.lines() {
        let dec = tap_decode::decode(TapStream::Trades, &TapData::Text(l.to_string())).unwrap();
        let TapView::Trades(list) = dec.view else {
            panic!("не сделка")
        };
        assert!(!list[0].id.is_empty() && list[0].time_ms > 0);
    }
}

#[test]
#[ignore = "приватная запись на живом боте: /account/balance — tests/README.md"]
fn recorded_balance_frames() {
    let t = std::fs::read_to_string(format!("{DIR}/kucoin_account_balance.jsonl"))
        .expect("запись баланса");
    for l in t.lines() {
        let depth::Msg::Data { data, .. } = depth::parse(l) else {
            continue;
        };
        arb::venues::kucoin::account_update::parse(&data, &VenueId("kucoin".into())).unwrap();
    }
}
