//! Файл: tests/bitget_real_data.rs
//! Что делает: адаптер Bitget (решение 162, Classic API v2) на официальном векторе подписи и на ЖИВЫХ ответах Bitget.
//! - подпись: Base64(HMAC-SHA256) — значения посчитаны функциями официального SDK github.com/BitgetLimited/
//!   v3-bitget-api-sdk (bitget-python-sdk-api/bitget/utils.py: pre_hash, sign; запуск — tests/README.md, раздел Bitget);
//!   строка запроса — как parse_params_to_str SDK (параметры по имени, без кодирования);
//! - правила своего ID (Best Practices: «^[0-9A-Za-z_:#\-+\s]{1,32}$») и размер запроса подписки (≤ 4096 байт);
//! - `#[ignore]` до записи с сервера бота (из среды разработки api.bitget.com закрыт прокси): справочник пар, монет и
//!   сетей (CAKE, USDT), стакан REST и кадры WebSocket books15, приватные ответы (all-trade-rate, orderInfo с feeDetail).
//!   Что и как записать — tests/README.md, раздел Bitget. Моков нет.
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::bitget.

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{Side, VenueId};
use arb::venues::bitget::{
    depth, fees, networks_parse, order_fee, order_rules, rest, sign, symbols, ws_conn,
};

const DIR: &str = "tests/data/cex_samples_2026_11_05";

/// data из записанного ответа REST Bitget ({code "00000", data}).
fn data(file: &str) -> Value {
    let t = std::fs::read_to_string(format!("{DIR}/{file}")).expect("файл записи");
    let mut v: Value = serde_json::from_str(&t).expect("JSON");
    assert_eq!(v["code"], "00000", "{file}: ответ не успешный");
    v.as_object_mut().unwrap().remove("data").unwrap()
}

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

#[test]
fn signature_matches_official_sdk_get() {
    // utils.sign(utils.pre_hash(1695808949356, 'GET', '/api/v2/spot/account/assets?coin=USDT', ''), 'test_api_secret')
    let plain = sign::prehash(
        1695808949356,
        "get",
        "/api/v2/spot/account/assets?coin=USDT",
        "",
    );
    assert_eq!(
        plain,
        "1695808949356GET/api/v2/spot/account/assets?coin=USDT"
    );
    let s = sign::hmac_b64(plain.as_bytes(), b"test_api_secret").unwrap();
    assert_eq!(s, "X+fN2ZKIaQNx8I+bkoc82/yTFv2yygsJ5/FLmL2+zIM=");
}

#[test]
fn signature_matches_official_sdk_post() {
    // utils.sign(utils.pre_hash(1695808949356, 'POST', '/api/v2/spot/trade/place-order',
    //   '{"symbol":"CAKEUSDT","side":"buy"}'), 'test_api_secret')
    let body = r#"{"symbol":"CAKEUSDT","side":"buy"}"#;
    let plain = sign::prehash(
        1695808949356,
        "POST",
        "/api/v2/spot/trade/place-order",
        body,
    );
    let s = sign::hmac_b64(plain.as_bytes(), b"test_api_secret").unwrap();
    assert_eq!(s, "qaU2/ofz+kDa9Q7uR9ZvV2gzgFKimI5edD9eTONAMeo=");
}

#[test]
fn query_sorted_like_sdk() {
    // utils.parse_params_to_str({'coin':'USDT','assetType':'all'}) == '?assetType=all&coin=USDT'
    let q = vec![
        ("coin", "USDT".to_string()),
        ("assetType", "all".to_string()),
    ];
    assert_eq!(rest::query(&q).unwrap(), "assetType=all&coin=USDT");
    // Знак, который потребовал бы кодирования, не уходит (строка в адресе и в подписи разошлась бы).
    assert!(rest::query(&vec![("coin", "a b".to_string())]).is_err());
}

#[test]
fn client_oid_fits_best_practices_rule() {
    let id = order_rules::client_oid('b', "deal-2026-11-05-very-long-identifier-0001");
    assert!(!id.is_empty() && id.len() <= 32, "{id}");
    assert!(
        id.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "{id}"
    );
    assert!(id.starts_with('b'));
}

#[test]
fn subscribe_messages_within_4096_bytes() {
    let syms: Vec<String> = (0..200).map(|i| format!("COIN{i:04}USDT")).collect();
    let refs: Vec<&str> = syms.iter().map(String::as_str).collect();
    let msgs = ws_conn::sub_messages("books15", &refs);
    assert!(msgs.len() > 1);
    let mut n = 0;
    for m in &msgs {
        assert!(m.len() <= ws_conn::MAX_SUB_BYTES, "{}", m.len());
        let v: Value = serde_json::from_str(m).unwrap();
        assert_eq!(v["op"], "subscribe");
        n += v["args"].as_array().unwrap().len();
    }
    assert_eq!(n, 200);
}

#[test]
#[ignore = "записать bitget_symbols.json — GET /api/v2/spot/public/symbols (tests/README.md, раздел Bitget)"]
fn recorded_symbols_cake() {
    let (map, skipped) = symbols::parse_list(&data("bitget_symbols.json"), "USDT").unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    let r = &map["CAKE"];
    assert_eq!(r.symbol, "CAKEUSDT");
    assert!(r.tick_size > Decimal::ZERO && r.step_size > Decimal::ZERO);
    // Правила ордера — та же функция у ордера и у check_order (решение 134).
    if r.tradable() {
        let (q, p) =
            order_rules::check_rules(r, Side::Buy, d("1000.123456789"), d("2.5"), "ioc").unwrap();
        assert!(q <= d("1000.123456789") && p <= d("2.5"));
        assert!(order_rules::check_rules(r, Side::Buy, d("1"), d("2.5"), "gtc").is_err());
    }
}

#[test]
#[ignore = "записать bitget_coins.json и bitget_coins_CAKE.json — GET /api/v2/spot/public/coins (tests/README.md)"]
fn recorded_coins_bsc() {
    let venue = VenueId("bitget".into());
    // Имя сети BSC — из живого ответа (venue.bsc_network): записать его в tests/README.md рядом с файлом.
    let bsc = std::fs::read_to_string(format!("{DIR}/bitget_bsc_network.txt"))
        .expect("bitget_bsc_network.txt")
        .trim()
        .to_string();
    for file in ["bitget_coins.json", "bitget_coins_CAKE.json"] {
        let p = networks_parse::parse(&data(file), &bsc, &venue, false).unwrap();
        let cake = p
            .list
            .iter()
            .find(|i| i.asset.ticker == "CAKE" && i.network == bsc)
            .expect("CAKE в сети BSC");
        let want: alloy::primitives::Address = "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82"
            .parse()
            .unwrap();
        assert_eq!(cake.asset.bsc_contract, Some(want), "{file}");
        assert!(cake.withdraw_fee.is_some() || p.extra_fee.iter().any(|x| x.starts_with("CAKE")));
    }
}

#[test]
#[ignore = "записать bitget_ws_books15.jsonl — кадры wss://ws.bitget.com/v2/ws/public (tests/README.md)"]
fn recorded_ws_frames() {
    let t = std::fs::read_to_string(format!("{DIR}/bitget_ws_books15.jsonl")).unwrap();
    let (mut subscribed, mut books, mut pongs) = (0, 0, 0);
    for line in t.lines().filter(|l| !l.trim().is_empty()) {
        match depth::parse(line) {
            depth::Msg::Subscribed(_) => subscribed += 1,
            depth::Msg::Pong => pongs += 1,
            depth::Msg::Depth { asks, bids, .. } => {
                books += 1;
                assert!(asks.windows(2).all(|w| w[0].price <= w[1].price));
                assert!(bids.windows(2).all(|w| w[0].price >= w[1].price));
            }
            depth::Msg::Bad { reason, .. } => panic!("кадр не разобран: {reason}: {line}"),
            _ => {}
        }
    }
    assert!(subscribed > 0 && books > 0 && pongs > 0);
}

#[test]
#[ignore = "записать на живом боте bitget_all_trade_rate.json — GET /api/v2/common/all-trade-rate?businessType=spot"]
fn recorded_all_trade_rate() {
    let m = fees::parse_all(&data("bitget_all_trade_rate.json")).unwrap();
    assert!(m.contains_key("CAKEUSDT"));
}

#[test]
#[ignore = "записать на живом боте bitget_order_info.json — orderInfo исполненного IOC-ордера (с feeDetail)"]
fn recorded_order_fee() {
    let v = data("bitget_order_info.json");
    let o = &v.as_array().unwrap()[0];
    let coin = o["baseCoin"].as_str().unwrap().to_ascii_uppercase();
    let fees = order_fee::parse(o.get("feeDetail"), &coin).unwrap();
    for f in &fees {
        assert!(f.amount > Decimal::ZERO);
        assert!(f.asset == coin || f.asset == order_fee::BGB);
    }
}

/// Решение 220: 40938 «Your account can only withdraw to addresses in the address book» (HTTP 400) — отказ вывода
/// (WithdrawRejected), как 11000, при любой операции. В официальной таблице кодов его нет: основание — живые ответы
/// Bitget на вывод из журнала бота (tests/data/bitget_40938_2026-11-07, сделки CHEEMS и др.).
#[test]
fn code_40938_is_withdraw_rejected() {
    use arb::gateway::errors::ErrorCode;
    use arb::venues::bitget::errors::{Op, from_code};
    let text = std::fs::read_to_string("tests/data/bitget_40938_2026-11-07/withdraw_rejected.jsonl")
        .expect("записанные ответы 40938");
    let mut n = 0;
    for raw in text.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(raw).expect("JSON ответа");
        let code = v["code"].as_str().expect("code — строка");
        let msg = v["msg"].as_str().expect("msg");
        assert!(v["data"].is_null());
        for op in [Op::Withdraw, Op::Move, Op::Other] {
            let e = from_code(400, code, msg, op, raw);
            assert_eq!(e.code, ErrorCode::WithdrawRejected, "{op:?}");
            assert!(e.message.contains("40938") && e.message.contains("address book"));
        }
        n += 1;
    }
    assert_eq!(n, 4);
    // 11000 — как раньше.
    assert_eq!(
        from_code(400, "11000", "", Op::Withdraw, "").code,
        ErrorCode::WithdrawRejected
    );
}
