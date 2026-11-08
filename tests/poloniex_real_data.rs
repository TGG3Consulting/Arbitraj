//! Файл: tests/poloniex_real_data.rs
//! Что делает: адаптер Poloniex (решение 169, Spot API) на официальном векторе подписи и на ЖИВЫХ ответах Poloniex.
//! - подпись REST и входа в приватный WebSocket: HmacSHA256 → Base64 — значения посчитаны функциями официального SDK
//!   github.com/poloniex/polo-sdk-python (polosdk/spot/rest/request.py: encode_uri_component, _get_sig_header;
//!   polosdk/spot/ws/client_authenticated.py: _get_signature; запуск — tests/README.md, раздел Poloniex);
//! - свой id ордера (17 цифр: Create Order «A-Za-z0-9_-», Error Codes 21315 «17 digits») и сообщение подписки book;
//! - `#[ignore]` до записи с сервера бота (из среды разработки api.poloniex.com закрыт прокси): справочник пар, монет и
//!   сетей (CAKE, USDT), кадры WebSocket book, приватные ответы (feeinfo, сделки ордера, история кошелька, кадры
//!   balances). Что и как записать — tests/README.md, раздел Poloniex. Моков нет.
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::poloniex.

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{Side, VenueId};
use arb::venues::poloniex::{
    account_update, activity, depth, fees, fills, networks_parse, order_rules, sign, symbols,
    ws_conn,
};

const DIR: &str = "tests/data/cex_samples_2026_11_05";
/// Секрет и метка времени, на которых посчитан вектор SDK (tests/README.md, раздел Poloniex).
const SECRET: &[u8] = b"test_api_secret";
const TS: i64 = 1631018760000;

/// Записанный ответ REST Poloniex (тело как есть — у успешного ответа обёртки нет).
fn data(file: &str) -> Value {
    let t = std::fs::read_to_string(format!("{DIR}/{file}")).expect("файл записи");
    serde_json::from_str(&t).expect("JSON")
}

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

#[test]
fn signature_get_matches_official_sdk() {
    let q = vec![
        ("symbol", "ETH_USDT".to_string()),
        ("limit", "5".to_string()),
    ];
    let plain = sign::payload("get", "/orders", &q, "", TS);
    assert_eq!(
        plain,
        "GET\n/orders\nlimit=5&signTimestamp=1631018760000&symbol=ETH_USDT"
    );
    let s = sign::hmac_b64(plain.as_bytes(), SECRET).unwrap();
    assert_eq!(s, "ep4qy3PmWRo/09xmVAjPvq42WD4zsJ+zCsvRs57s1tg=");
}

#[test]
fn signature_post_body_matches_official_sdk() {
    let body = r#"{"symbol":"CAKE_USDT","side":"BUY"}"#;
    let plain = sign::payload("POST", "/orders", &[], body, TS);
    let s = sign::hmac_b64(plain.as_bytes(), SECRET).unwrap();
    assert_eq!(s, "y9HJUDmcJ9UCNzDkaj0QBTSnmKvz4vN7AvgIEWXj1u0=");
}

#[test]
fn signature_delete_without_body_matches_official_sdk() {
    let plain = sign::payload("DELETE", "/orders/1", &[], "", TS);
    assert_eq!(plain, "DELETE\n/orders/1\nsignTimestamp=1631018760000");
    let s = sign::hmac_b64(plain.as_bytes(), SECRET).unwrap();
    assert_eq!(s, "IFy5lcO0DQCLW3GFYfQrTBwWOQXdShyR7FODJNDi/T0=");
}

#[test]
fn signature_ws_auth_matches_official_sdk() {
    let s = sign::hmac_b64(sign::ws_payload(TS).as_bytes(), SECRET).unwrap();
    assert_eq!(s, "YF1aTjKrIPHxQV/kRLH/J2wPqIipIcEjzo8CQfI+7UE=");
}

#[test]
fn encoding_like_sdk() {
    // encode_uri_component('a b/c~(x)') == 'a%20b%2Fc~(x)' (urllib.parse.quote, safe="~()*!'")
    assert_eq!(sign::encode("a b/c~(x)"), "a%20b%2Fc~(x)");
}

#[test]
fn client_id_fits_both_rules() {
    let (a, b) = (order_rules::client_id(), order_rules::client_id());
    assert_ne!(a, b, "новый id у каждой попытки");
    for id in [a, b] {
        assert_eq!(id.len(), order_rules::CLIENT_ID_LEN, "{id}");
        assert!(id.chars().all(|c| c.is_ascii_digit()), "{id}");
    }
}

#[test]
fn subscribe_message_book() {
    let v: Value = serde_json::from_str(&ws_conn::sub_message("CAKE_USDT", 20)).unwrap();
    assert_eq!(v["event"], "subscribe");
    assert_eq!(v["channel"][0], "book");
    assert_eq!(v["symbols"][0], "CAKE_USDT");
    assert_eq!(v["depth"], 20);
}

#[test]
#[ignore = "записать poloniex_markets.json — GET /markets (tests/README.md, раздел Poloniex)"]
fn recorded_markets_cake() {
    let (map, skipped) = symbols::parse_list(&data("poloniex_markets.json"), "USDT").unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    let r = &map["CAKE"];
    assert_eq!(r.symbol, "CAKE_USDT");
    assert!(r.tick_size > Decimal::ZERO && r.step_size > Decimal::ZERO);
    // Правила ордера — та же функция у ордера и у check_order (решение 134).
    if r.tradable() {
        let (q, p) =
            order_rules::check_rules(r, Side::Buy, d("1000.123456789"), d("2.5"), "IOC").unwrap();
        assert!(q <= d("1000.123456789") && p <= d("2.5"));
        assert!(order_rules::check_rules(r, Side::Buy, d("1000"), d("2.5"), "GTC").is_err());
    }
}

#[test]
#[ignore = "записать poloniex_currencies_v2.json и poloniex_currencies_v2_CAKE.json — GET /v2/currencies (tests/README.md)"]
fn recorded_currencies_bsc() {
    let venue = VenueId("poloniex".into());
    // Имя сети BSC (blockchain) — из живого ответа (venue.bsc_network): записать в poloniex_bsc_network.txt.
    let bsc = std::fs::read_to_string(format!("{DIR}/poloniex_bsc_network.txt"))
        .expect("poloniex_bsc_network.txt")
        .trim()
        .to_string();
    let want: alloy::primitives::Address = "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82"
        .parse()
        .unwrap();
    for file in [
        "poloniex_currencies_v2.json",
        "poloniex_currencies_v2_CAKE.json",
    ] {
        let raw = data(file);
        let p = networks_parse::parse(&raw, &bsc, &venue, false).unwrap();
        let cake = p
            .list
            .iter()
            .find(|i| i.asset.ticker == "CAKE" && i.network == bsc)
            .expect("CAKE в сети BSC");
        assert_eq!(cake.asset.bsc_contract, Some(want), "{file}");
        assert!(cake.withdraw_fee.is_some(), "{file}");
        let (coin, _) = networks_parse::chain_coin(&raw, "CAKE", &bsc).expect("имя монеты в сети");
        assert!(!coin.is_empty());
    }
}

#[test]
#[ignore = "записать poloniex_ws_book.jsonl — кадры wss://ws.poloniex.com/ws/public (tests/README.md)"]
fn recorded_ws_book_frames() {
    let t = std::fs::read_to_string(format!("{DIR}/poloniex_ws_book.jsonl")).unwrap();
    let (mut subscribed, mut books, mut pongs) = (0, 0, 0);
    for line in t.lines().filter(|l| !l.trim().is_empty()) {
        match depth::parse(line) {
            depth::Msg::Subscribed => subscribed += 1,
            depth::Msg::Pong => pongs += 1,
            depth::Msg::Books(list) => {
                for b in list {
                    books += 1;
                    assert!(b.asks.windows(2).all(|w| w[0].price <= w[1].price));
                    assert!(b.bids.windows(2).all(|w| w[0].price >= w[1].price));
                }
            }
            depth::Msg::Bad { reason, .. } => panic!("кадр не разобран: {reason}: {line}"),
            _ => {}
        }
    }
    assert!(subscribed > 0 && books > 0 && pongs > 0);
}

#[test]
#[ignore = "записать на живом боте poloniex_feeinfo.json — GET /feeinfo"]
fn recorded_feeinfo() {
    let i = fees::parse(&data("poloniex_feeinfo.json")).unwrap();
    assert!(i.of("CAKE_USDT").taker >= Decimal::ZERO);
}

#[test]
#[ignore = "записать на живом боте poloniex_order_trades.json — GET /orders/{id}/trades исполненного IOC-ордера"]
fn recorded_order_trades() {
    let (qty, fees) = fills::sum(&data("poloniex_order_trades.json")).unwrap();
    assert!(qty > Decimal::ZERO && fees.iter().all(|f| !f.asset.is_empty()));
}

#[test]
#[ignore = "записать на живом боте poloniex_wallets_activity.json — GET /wallets/activity (депозит и вывод CAKE)"]
fn recorded_wallet_activity() {
    let v = data("poloniex_wallets_activity.json");
    let dep = activity::list(&v, activity::DEPOSITS).unwrap();
    let wd = activity::list(&v, activity::WITHDRAWALS).unwrap();
    assert!(!dep.is_empty() || !wd.is_empty());
    for r in dep.iter().chain(wd.iter()) {
        assert!(r.get("status").and_then(Value::as_str).is_some(), "{r}");
    }
}

#[test]
#[ignore = "записать на живом боте poloniex_ws_balances.jsonl — кадры wss://ws.poloniex.com/ws/private (auth, balances)"]
fn recorded_ws_balances() {
    let t = std::fs::read_to_string(format!("{DIR}/poloniex_ws_balances.jsonl")).unwrap();
    let (mut auth, mut rows) = (0, 0);
    for line in t.lines().filter(|l| !l.trim().is_empty()) {
        match account_update::parse(line) {
            account_update::Frame::Auth(Ok(())) => auth += 1,
            account_update::Frame::Balances(r) => rows += r.len(),
            account_update::Frame::Bad(why) => panic!("кадр не разобран: {why}: {line}"),
            _ => {}
        }
    }
    assert!(auth > 0 && rows > 0);
}
