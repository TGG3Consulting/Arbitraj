//! Файл: tests/phemex_real_data.rs
//! Что делает: адаптер Phemex (решение 168, только спот) на официальных примерах подписи и на ЖИВЫХ ответах Phemex.
//! - подпись: строка для подписи — ровно «Signed string» примеров 2 и 3 официальной документации (General API
//!   information → Signature example); HMAC SHA256 → hex — значения посчитаны теми же вызовами, что в официальном
//!   примере клиента github.com/phemex/phemex-python-api (client.py: hmac.new(secret, message, sha256).hexdigest()),
//!   команда — tests/README.md, раздел Phemex; подпись входа WebSocket — «HMacSha256(API Key + expiry)» на ключе и expiry
//!   из примера user.auth;
//! - строка запроса — параметры в заданном порядке без кодирования (как client.py);
//! - перевод масштабированных целых — на числах таблицы «All Spot trading products» (sBTCUSDT: quoteTickSizeEv
//!   1000000 = 0.01 USDT при priceScale 8, baseTickSizeEv 100 = 0.000001 BTC при valueScale 8);
//! - `#[ignore]` до записи с сервера бота (из среды разработки api.phemex.com закрыт прокси): справочник пар и монет,
//!   стакан REST и кадры WebSocket, приватные ответы. Что и как записать — tests/README.md, раздел Phemex. Моков нет.
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::phemex.

use rust_decimal::Decimal;
use serde_json::Value;

use arb::venues::phemex::{
    depth, fees, local_book, num, order_rules, rest, sign, symbols, ws_conn,
};

const DIR: &str = "tests/data/cex_samples_2026_11_05";

/// Записанный ответ REST Phemex целиком.
fn file(name: &str) -> Value {
    let t = std::fs::read_to_string(format!("{DIR}/{name}")).expect("файл записи");
    serde_json::from_str(&t).expect("JSON")
}

/// data из записанного ответа REST Phemex ({code 0, data}).
fn data(name: &str) -> Value {
    let mut v = file(name);
    assert_eq!(v["code"], 0, "{name}: ответ не успешный");
    v.as_object_mut().unwrap().remove("data").unwrap()
}

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

#[test]
fn signed_string_matches_official_example_2() {
    let q = "ordStatus=New&ordStatus=PartiallyFilled&ordStatus=Untriggered&symbol=BTCUSD";
    let plain = sign::prehash("/orders/activeList", q, 1575735951, "");
    assert_eq!(
        plain,
        "/orders/activeListordStatus=New&ordStatus=PartiallyFilled&ordStatus=Untriggered&symbol=BTCUSD1575735951"
    );
    // hmac.new(b'test_api_secret', plain.encode(), hashlib.sha256).hexdigest()
    let s = sign::hmac_hex(plain.as_bytes(), b"test_api_secret").unwrap();
    assert_eq!(
        s,
        "09a06e3d4152159bdb6db5a66791a4f3b920e8b1b150da723cef2ce0d4b8619e"
    );
}

#[test]
fn signed_string_matches_official_example_1_and_3() {
    let get = sign::prehash("/accounts/accountPositions", "currency=BTC", 1575735514, "");
    assert_eq!(get, "/accounts/accountPositionscurrency=BTC1575735514");
    let s = sign::hmac_hex(get.as_bytes(), b"test_api_secret").unwrap();
    assert_eq!(
        s,
        "9e4559ee30deabfb877f26e2d9e039d2133782b44f7e553a1576c6dd261db3b8"
    );
    let body = r#"{"symbol":"BTCUSD","clOrdID":"uuid-1573058952273","side":"Sell","priceEp":93185000,"orderQty":7,"ordType":"Limit","reduceOnly":false,"timeInForce":"GoodTillCancel","takeProfitEp":0,"stopLossEp":0}"#;
    let post = sign::prehash("/orders", "", 1575735514, body);
    assert_eq!(post, format!("/orders1575735514{body}"));
    let s = sign::hmac_hex(post.as_bytes(), b"test_api_secret").unwrap();
    assert_eq!(
        s,
        "d68d7eddd096d3b77deec362a778de184ac34cba09953179030c3c81fe8e34aa"
    );
}

#[test]
fn ws_auth_signs_key_plus_expiry() {
    let keys = arb::venues::phemex::ctx::Keys {
        key: "806066b0-f02b-4d3e-b444-76ec718e1023".to_string().into(),
        secret: "test_api_secret".to_string().into(),
    };
    let m = sign::ws_auth(&keys, 1570091232, 1).unwrap();
    assert_eq!(m["method"], "user.auth");
    assert_eq!(m["params"][0], "API");
    assert_eq!(m["params"][1], "806066b0-f02b-4d3e-b444-76ec718e1023");
    assert_eq!(
        m["params"][2],
        "0b9b179895d7b7acdde09bbbeab513da777498233cfe17328b6437978c2314ba"
    );
    assert_eq!(m["params"][3], 1570091232);
}

#[test]
fn query_keeps_order_without_encoding() {
    let q = vec![
        ("symbol", "sCAKEUSDT".to_string()),
        ("clOrdID", "a-1".to_string()),
    ];
    assert_eq!(rest::query(&q).unwrap(), "symbol=sCAKEUSDT&clOrdID=a-1");
    assert!(rest::query(&vec![("currency", "a b".to_string())]).is_err());
    assert!(rest::query(&vec![("currency", String::new())]).is_err());
}

#[test]
fn scales_match_official_product_table() {
    assert_eq!(num::unscale(1_000_000, 8), Some(d("0.01")));
    assert_eq!(num::unscale(100, 8), Some(d("0.000001")));
    assert_eq!(num::scale_up(d("10"), 8), Ok(1_000_000_000));
    assert_eq!(num::scale_up(d("0.000001"), 8), Ok(100));
    // Лишний знак не округляется молча — ордер в 10^k раз не уйдёт.
    assert!(num::scale_up(d("0.000000001"), 8).is_err());
}

#[test]
fn own_order_id_is_uuid_v4_and_new_each_time() {
    let a = order_rules::uuid4().unwrap();
    let b = order_rules::uuid4().unwrap();
    assert_ne!(a, b);
    let parts: Vec<&str> = a.split('-').collect();
    assert_eq!(
        parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12]
    );
    assert!(parts[2].starts_with('4'));
}

#[test]
fn subscribe_message_is_official_shape() {
    let v: Value = serde_json::from_str(&ws_conn::sub_message(3, "sCAKEUSDT")).unwrap();
    assert_eq!(v["method"], "orderbook.subscribe");
    assert_eq!(v["params"][0], "sCAKEUSDT");
    assert_eq!(v["id"], 3);
}

#[test]
#[ignore = "нужна запись phemex_products.json (tests/README.md, раздел Phemex)"]
fn products_parse_live() {
    let p = symbols::parse_products(&data("phemex_products.json"), "USDT", "Listed").unwrap();
    let cake = p.pairs.get("CAKE").expect("sCAKEUSDT");
    assert_eq!(cake.symbol, "sCAKEUSDT");
    assert!(cake.tick_size > Decimal::ZERO && cake.step_size > Decimal::ZERO);
    assert!(p.scales.contains_key("USDT"));
    assert!(p.skipped.is_empty(), "{:?}", p.skipped);
}

#[test]
#[ignore = "нужна запись phemex_ws_orderbook_sCAKEUSDT.jsonl (tests/README.md, раздел Phemex)"]
fn ws_orderbook_frames_live() {
    let t = std::fs::read_to_string(format!("{DIR}/phemex_ws_orderbook_sCAKEUSDT.jsonl")).unwrap();
    let mut lb = local_book::LocalBook::default();
    let mut books = 0;
    for line in t.lines() {
        if let depth::Msg::Book(f) = depth::parse(line) {
            books += 1;
            let a = lb.apply(&f, false);
            assert_ne!(a, local_book::Applied::Waiting, "изменение до снимка");
            assert!(!matches!(a, local_book::Applied::Gap(_)), "{a:?}");
            let (bids, asks) = lb.top(30, 8, 8);
            assert!(bids.windows(2).all(|w| w[0].price > w[1].price));
            assert!(asks.windows(2).all(|w| w[0].price < w[1].price));
        }
    }
    assert!(books > 1);
}

#[test]
#[ignore = "нужна приватная запись phemex_fee_rate.json с сервера бота (tests/README.md, раздел Phemex)"]
fn fee_rate_parse_live() {
    let r = fees::parse(&data("phemex_fee_rate.json")).unwrap();
    assert!(r.contains_key("sCAKEUSDT"));
}

/// Решения 202/213 для всех бирж (Тигран 08.11), venues/phemex/book_print.rs — подтверждение тихой пары по содержимому.
/// Разбор result GET /md/orderbook на ПРИМЕРЕ ИЗ ДОКУМЕНТАЦИИ (Public-Spot-API-en.md, «Query Order Book»: sBTCUSDT,
/// asks [[877050000000, 1000000], [877100000000, 200000]], bids [[877000000000, 2000000], [876950000000, 200000]],
/// sequence 455476965) с масштабами 8 / 8 (как в scales_match_official_product_table) — живого ответа и кадров в tests/data
/// нет (api.phemex.com закрыт прокси; что записать — tests/README.md, раздел Phemex). Стакан в памяти — локальный стакан
/// из снимка OrderBook WebSocket того же вида, что в документации, с уровнями примера REST (собран здесь, не запись).
/// Сравнение — живая логика ядра (core/feed_confirm.rs verdict): тот же отпечаток — Quiet; стакана нет или он другой —
/// Lost.
#[test]
fn book_print_docs_example() {
    use arb::core::feed_confirm::{Verdict, verdict};
    use arb::gateway::feed_health::{BookPrint, BookPrints, BookVersion, PairHealth};
    use arb::venues::phemex::book_print;
    let result: Value = serde_json::from_str(
        r#"{"book":{"asks":[[877050000000,1000000],[877100000000,200000]],"bids":[[877000000000,2000000],[876950000000,200000]]},"depth":30,"sequence":455476965,"timestamp":1583555482434235628,"symbol":"sBTCUSDT","type":"snapshot"}"#,
    )
    .unwrap();
    let p = book_print::parse(&result, 30, 8, 8).unwrap();
    assert_eq!((p.bids.len(), p.asks.len()), (2, 2));
    assert_eq!((p.asks[0].price, p.asks[0].qty), (d("8770.5"), d("0.01")));
    assert_eq!((p.bids[0].price, p.bids[0].qty), (d("8770"), d("0.02")));
    assert_eq!(p.bids[1].price, d("8769.5"));
    // Тот же стакан из снимка WebSocket (локальный стакан, ws_conn.rs).
    let frame = r#"{"book":{"asks":[[877050000000,1000000],[877100000000,200000]],"bids":[[877000000000,2000000],[876950000000,200000]]},"depth":30,"sequence":455476965,"symbol":"sBTCUSDT","timestamp":1583555482434235628,"type":"snapshot"}"#;
    let depth::Msg::Book(f) = depth::parse(frame) else {
        panic!("кадр OrderBook не разобран")
    };
    let mut lb = local_book::LocalBook::default();
    assert_eq!(lb.apply(&f, false), local_book::Applied::Changed);
    let (bids, asks) = lb.top(30, 8, 8);
    let ours = BookPrint::top(&bids, &asks, 30);
    let pair = PairHealth {
        ticker: "BTC".into(),
        last_ms: Some(1),
        since_ms: 0,
        last_id: None,
        last_changed_ms: None,
    };
    let ver = |ours: Option<BookPrint>| BookVersion {
        print: Some(BookPrints {
            levels: 30,
            rest: p.clone(),
            ours,
        }),
        ..Default::default()
    };
    assert_eq!(verdict(&pair, &ver(Some(ours)), 2), Verdict::Quiet);
    assert!(matches!(verdict(&pair, &ver(None), 2), Verdict::Lost(_)));
    let one = book_print::parse(&result, 1, 8, 8).unwrap();
    assert!(matches!(
        verdict(&pair, &ver(Some(one)), 2),
        Verdict::Lost(_)
    ));
    // Вид ответа: нет book, нет bids.
    assert_eq!(
        book_print::parse(&Value::Null, 30, 8, 8).unwrap_err(),
        "нет book"
    );
    let bad: Value = serde_json::from_str(r#"{"book":{"asks":[]}}"#).unwrap();
    assert_eq!(book_print::parse(&bad, 30, 8, 8).unwrap_err(), "нет bids");
}

/// Решение 202: отписка — вид «Unsubscribe OrderBook» (params пустой — снимает все стаканы соединения).
#[test]
fn unsubscribe_message_is_official_shape() {
    let v: Value = serde_json::from_str(&ws_conn::unsub_message(21)).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"id": 21, "method": "orderbook.unsubscribe", "params": []})
    );
}
