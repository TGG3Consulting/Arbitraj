//! Файл: tests/coinex_real_data.rs
//! Что делает: адаптер CoinEx (решение 170, API v2) на официальных строках документации и на ЖИВЫХ ответах CoinEx.
//! - подпись: строка для подписи — пример страницы Authentication («GET» + «/v2/spot/pending-order?…» + «1700490703564»);
//!   готовой подписи с ключом на странице нет (ключи «XXXXXXXXXX»), поэтому HMAC-SHA256 → hex строчными проверяется на
//!   официальном векторе RFC 4231 (Test Case 2) — тот же hmac.new(…, hashlib.sha256).hexdigest().lower() документации;
//! - строка запроса (порядок параметров, запрет знаков, требующих кодирования), правило client_id (≤ 32 байт, буквы,
//!   цифры, «-», «_»), запрос depth.subscribe, распознавание вида сжатия кадра (заголовки RFC 1950 / 1952);
//! - `#[ignore]` до записи с сервера бота (из среды разработки api.coinex.com закрыт прокси): справочник пар, монет
//!   и сетей с адресом контракта (CAKE, USDT), кадры WebSocket depth.update (сжатые, base64 по строке), приватные
//!   ответы (trade-fee-rate, ордер IOC, user-deals). Что и как записать — tests/README.md, раздел CoinEx. Моков нет.
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::coinex.

use std::collections::HashMap;

use base64::Engine as _;
use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{Side, VenueId};
use arb::venues::coinex::{
    contracts, deals, depth, fees, inflate, networks_parse, order_fee, order_rules, rest, sign,
    symbols, ws_conn,
};

const DIR: &str = "tests/data/cex_samples_2026_11_05";

/// data из записанного ответа REST CoinEx ({code 0, data, message}).
fn data(file: &str) -> Value {
    let t = std::fs::read_to_string(format!("{DIR}/{file}")).expect("файл записи");
    let v: Value = serde_json::from_str(&t).expect("JSON");
    assert_eq!(v["code"], 0, "{file}: ответ не успешный");
    rest::data(v)
}

/// Строка из файла-пометки (имя сети и т. п.).
fn note(file: &str) -> String {
    std::fs::read_to_string(format!("{DIR}/{file}"))
        .expect(file)
        .trim()
        .to_string()
}

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

#[test]
fn prehash_matches_documentation_example() {
    let path = "/v2/spot/pending-order?market=BTCUSDT&market_type=SPOT&side=buy&page=1&limit=10";
    let plain = sign::prehash("get", path, "", 1700490703564);
    assert_eq!(plain, format!("GET{path}1700490703564"));
    // POST: тело между путём и временем (тело примера страницы).
    let body = r#"{"market": "BTCUSDT", "type": "buy", "amount": "0.001", "price": "10000"}"#;
    let plain = sign::prehash("POST", "/v2/spot/order", body, 1700490703564);
    assert_eq!(plain, format!("POST/v2/spot/order{body}1700490703564"));
}

#[test]
fn hmac_hex_rfc4231_case2() {
    // RFC 4231, 4.3 Test Case 2: Key = "Jefe", Data = "what do ya want for nothing?", HMAC-SHA-256.
    let s = sign::hmac_hex(b"what do ya want for nothing?", b"Jefe").unwrap();
    assert_eq!(
        s,
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    assert!(
        s.len() == 64
            && s.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
}

#[test]
fn query_keeps_order_and_rejects_unsafe() {
    let q = vec![
        ("market", "CAKEUSDT".to_string()),
        ("market_type", "SPOT".to_string()),
    ];
    assert_eq!(rest::query(&q).unwrap(), "market=CAKEUSDT&market_type=SPOT");
    assert!(rest::query(&vec![("ccy", "a b".to_string())]).is_err());
}

#[test]
fn client_id_fits_documentation_rule() {
    let id = order_rules::client_id('b', "deal-2026-11-05-very-long-identifier-0001");
    assert!(
        !id.is_empty() && id.len() <= order_rules::MAX_CLIENT_ID,
        "{id}"
    );
    assert!(
        id.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
        "{id}"
    );
}

#[test]
fn depth_subscribe_request_shape() {
    let m: Value =
        serde_json::from_str(&ws_conn::sub_message(&["CAKEUSDT", "BTCUSDT"], 20)).unwrap();
    assert_eq!(m["method"], "depth.subscribe");
    let list = m["params"]["market_list"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0], serde_json::json!(["CAKEUSDT", 20, "0", true]));
}

#[test]
fn compression_format_by_header() {
    // RFC 1952: ID1 = 0x1f, ID2 = 0x8b; RFC 1950: 0x78 0x9c — CM = 8, (0x78·256 + 0x9c) кратно 31.
    assert_eq!(
        inflate::Format::of(&[0x1f, 0x8b, 8, 0]),
        inflate::Format::Gzip
    );
    assert_eq!(inflate::Format::of(&[0x78, 0x9c, 0]), inflate::Format::Zlib);
    assert_eq!(
        inflate::Format::of(&[0x78, 0x9d, 0]),
        inflate::Format::Deflate
    );
}

#[test]
#[ignore = "записать coinex_market.json — GET /v2/spot/market (tests/README.md, раздел CoinEx)"]
fn recorded_market_cake() {
    let (map, skipped) = symbols::parse_list(&data("coinex_market.json"), "USDT").unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    let r = &map["CAKE"];
    assert_eq!(r.symbol, "CAKEUSDT");
    assert!(r.tick_size > Decimal::ZERO && r.step_size > Decimal::ZERO);
    if r.tradable() {
        let (q, p) =
            order_rules::check_rules(r, Side::Buy, d("1000.123456789"), d("2.5"), "ioc").unwrap();
        assert!(q <= d("1000.123456789") && p <= d("2.5"));
        assert!(order_rules::check_rules(r, Side::Buy, d("1000"), d("2.5"), "limit").is_err());
    }
}

#[test]
#[ignore = "записать coinex_assets_info_CAKE.json, coinex_dw_config_CAKE.json, coinex_bsc_network.txt, coinex_bsc_contract_chain.txt (tests/README.md)"]
fn recorded_cake_contract_and_network() {
    let venue = VenueId("coinex".into());
    let c = contracts::parse(
        &data("coinex_assets_info_CAKE.json"),
        &note("coinex_bsc_contract_chain.txt"),
    )
    .unwrap();
    let want: alloy::primitives::Address = "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82"
        .parse()
        .unwrap();
    assert_eq!(c.by_ticker.get("CAKE"), Some(&want), "bad: {:?}", c.bad);
    let bsc = note("coinex_bsc_network.txt");
    let p = networks_parse::parse(
        &data("coinex_dw_config_CAKE.json"),
        &bsc,
        &c.by_ticker,
        &venue,
        false,
    )
    .unwrap();
    let cake = p
        .list
        .iter()
        .find(|i| i.network == bsc)
        .expect("CAKE в сети BSC");
    assert_eq!(cake.asset.bsc_contract, Some(want));
    assert!(cake.withdraw_fee.is_some() || !p.deflation.is_empty());
}

#[test]
#[ignore = "записать coinex_ws_depth.b64 — кадры wss://socket.coinex.com/v2/spot, base64 по строке (tests/README.md)"]
fn recorded_ws_depth_frames() {
    let t = std::fs::read_to_string(format!("{DIR}/coinex_ws_depth.b64")).unwrap();
    let (mut zip, mut books, mut acks, mut pongs) = (inflate::Inflater::default(), 0, 0, 0);
    for line in t.lines().filter(|l| !l.trim().is_empty()) {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(line.trim())
            .unwrap();
        let (text, _) = zip.text(&raw, 1 << 22).unwrap();
        match depth::parse(text) {
            depth::Msg::Ack(_) => acks += 1,
            depth::Msg::Pong => pongs += 1,
            depth::Msg::Depth {
                full, asks, bids, ..
            } => {
                books += 1;
                assert!(full, "при if_full = true пуш должен быть полным: {text}");
                assert!(asks.windows(2).all(|w| w[0].price <= w[1].price));
                assert!(bids.windows(2).all(|w| w[0].price >= w[1].price));
            }
            depth::Msg::Bad { reason, .. } => panic!("кадр не разобран: {reason}: {text}"),
            _ => {}
        }
    }
    assert!(acks > 0 && books > 0 && pongs > 0);
}

#[test]
#[ignore = "записать на живом боте coinex_trade_fee_rate.json — GET /v2/account/trade-fee-rate?market_type=SPOT&market=CAKEUSDT"]
fn recorded_trade_fee_rate() {
    let r = fees::parse(&data("coinex_trade_fee_rate.json"), "CAKEUSDT", false).unwrap();
    assert!(r.taker >= Decimal::ZERO && r.maker >= Decimal::ZERO);
}

#[test]
#[ignore = "записать на живом боте coinex_order_status.json и coinex_user_deals.json одного исполненного IOC-ордера"]
fn recorded_order_fee_and_deals() {
    let s = data("coinex_order_status.json");
    let fee = order_fee::from_order(&s, "CAKE", "USDT", "CAKE").unwrap();
    let id = s["order_id"].to_string();
    let mut sum = deals::Sum::default();
    deals::add(&mut sum, &data("coinex_user_deals.json"), &id).unwrap();
    assert!(sum.count > 0);
    assert_eq!(Some(sum.filled), s["filled_amount"].as_str().map(d));
    if let order_fee::Fee::Known(f, c) = fee {
        let by: HashMap<String, Decimal> = sum.fees.into_iter().collect();
        assert_eq!(by.get(&c).copied().unwrap_or_default(), f);
    }
}

/// Решение 202 (Тигран 08.11), venues/coinex/ws_msg.rs: переподписка сторожем — depth.unsubscribe всех рынков (пример
/// документации WS Market Depth «All markets»: {"method": "depth.unsubscribe", "params": {"market_list": []}, "id": 1}) и
/// depth.subscribe всех рынков соединения со своим id (отказ разбирается по id).
#[test]
fn resub_request_shape() {
    use arb::venues::coinex::ws_msg;
    let un: Value = serde_json::from_str(&ws_msg::unsub_all_message(1)).unwrap();
    assert_eq!(
        un,
        serde_json::json!({"method": "depth.unsubscribe", "params": {"market_list": []}, "id": 1})
    );
    let sub: Value =
        serde_json::from_str(&ws_msg::sub_message_id(&["CAKEUSDT", "BTCUSDT"], 20, 7)).unwrap();
    assert_eq!(sub["method"], "depth.subscribe");
    assert_eq!(sub["id"], 7);
    assert_eq!(
        sub["params"]["market_list"][1],
        serde_json::json!(["BTCUSDT", 20, "0", true])
    );
}

/// Решение 213 (Тигран 08.11), venues/coinex/book_print.rs — подтверждение тихой пары по содержимому. Разбор ответа
/// GET /v2/spot/depth на ПРИМЕРЕ ИЗ ДОКУМЕНТАЦИИ (Get Market Depth, Response example: market BTCUSDT, is_full true, asks
/// [["30740.00","0.31763545"],["30769.00","1.45155000"]], bids [["30736.00","0.04857373"],["30733.00","0.84696320"],
/// ["30725.00","0.12563353"],["30422.00","0"]], last, updated_at 1689152421692, checksum 2578768879; вложенность data →
/// depth — по таблице полей) — живого ответа и кадров в tests/data нет (api.coinex.com закрыт прокси; что записать —
/// tests/README.md). Уровень с количеством "0" не берётся (как у пуша, depth.rs). Тот же объект depth, разобранный
/// разбором пуша (depth::levels), даёт тот же отпечаток — verdict ядра Quiet; стакана в памяти нет или другой — Lost.
#[test]
fn book_print_docs_example() {
    use arb::core::feed_confirm::{Verdict, verdict};
    use arb::gateway::feed_health::{BookPrint, BookPrints, BookVersion, PairHealth};
    use arb::venues::coinex::book_print;
    let body: Value = serde_json::from_str(
        r#"{"code":0,"data":{"market":"BTCUSDT","is_full":true,"depth":{"asks":[["30740.00","0.31763545"],["30769.00","1.45155000"]],"bids":[["30736.00","0.04857373"],["30733.00","0.84696320"],["30725.00","0.12563353"],["30422.00","0"]],"last":"30746.28","updated_at":1689152421692,"checksum":2578768879}},"message":"OK"}"#,
    )
    .unwrap();
    let data = rest::data(body);
    let p = book_print::parse(&data, 5).unwrap();
    assert_eq!((p.bids.len(), p.asks.len()), (3, 2));
    assert_eq!(
        (p.bids[0].price, p.bids[0].qty),
        (d("30736.00"), d("0.04857373"))
    );
    assert_eq!(
        (p.asks[0].price, p.asks[0].qty),
        (d("30740.00"), d("0.31763545"))
    );
    assert_eq!(book_print::parse(&data, 2).unwrap().bids.len(), 2);
    // Разбор пуша (depth.rs) того же объекта depth.
    let dep = &data["depth"];
    let mut bids = depth::levels(dep, "bids").unwrap();
    let mut asks = depth::levels(dep, "asks").unwrap();
    bids.sort_by_key(|b| std::cmp::Reverse(b.price));
    asks.sort_by_key(|a| a.price);
    let ours = BookPrint::top(&bids, &asks, 5);
    let pair = PairHealth {
        ticker: "BTC".into(),
        last_ms: Some(1),
        since_ms: 0,
        last_id: None,
        last_changed_ms: None,
    };
    let ver = |ours: Option<BookPrint>| BookVersion {
        print: Some(BookPrints {
            levels: 5,
            rest: p.clone(),
            ours,
        }),
        ..Default::default()
    };
    assert_eq!(verdict(&pair, &ver(Some(ours)), 2), Verdict::Quiet);
    assert!(matches!(verdict(&pair, &ver(None), 2), Verdict::Lost(_)));
    let other = BookPrint::top(&bids, &asks, 1);
    assert!(matches!(
        verdict(&pair, &ver(Some(other)), 2),
        Verdict::Lost(_)
    ));
    let bad: Value = serde_json::from_str(r#"{"market":"BTCUSDT"}"#).unwrap();
    assert_eq!(book_print::parse(&bad, 5).unwrap_err(), "нет data.depth");
}
