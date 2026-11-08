//! Файл: tests/htx_real_data.rs
//! Что делает: адаптер HTX (решение 171, Spot API v1/v2) на официальных векторах подписи и на ЖИВЫХ ответах HTX.
//! - подпись: векторы тестов официального SDK github.com/HuobiRDCenter/huobi_Golang (internal/requestbuilder:
//!   signer_test.go, privateurlbuilder_test.go, websocketv2requestbuilder_test.go) — те же входы, те же подписи;
//! - правила своего ID (ордер ≤ 64, вывод ≤ 32 знаков: буквы, цифры, «_», «-»), тип ордера по сроку действия;
//! - `#[ignore]` до записи с сервера бота (из среды разработки api.huobi.pro закрыт прокси): справочник пар, монет и
//!   сетей (CAKE, USDT), справочник сетей с ac / ca (адрес контракта), кадры WebSocket mbp.refresh, приватные ответы
//!   (transact-fee-rate, matchresults, балансы, история депозитов, кадры accounts.update#2). Что и как записать —
//!   tests/README.md, раздел HTX. Моков нет.
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::htx.

use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::Value;
use zeroize::Zeroizing;

use arb::core::types::{Side, VenueId};
use arb::venues::htx::ctx::Keys;
use arb::venues::htx::{
    account_update, deposits, depth, fees, networks_parse, order_fee, order_rules, reply, sign,
    symbols, transfers,
};

const DIR: &str = "tests/data/cex_samples_2026_11_05";
/// Адрес CAKE в BSC (PancakeSwap Token, BscScan).
const CAKE: &str = "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82";

/// data из записанного ответа REST HTX (v1 status "ok" или v2 code 200).
fn data(file: &str) -> Value {
    let t = std::fs::read_to_string(format!("{DIR}/{file}")).expect("файл записи");
    let mut v: Value = serde_json::from_str(&t).expect("JSON");
    assert!(reply::success(&v), "{file}: ответ не успешный");
    v.as_object_mut().unwrap().remove("data").unwrap()
}

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Ключи тестов SDK: Access Key «access», Secret Key «secret».
fn sdk_keys() -> Keys {
    Keys {
        key: Zeroizing::new("access".into()),
        secret: Zeroizing::new("secret".into()),
    }
}

/// 2019-11-21T10:00:00 UTC — время тестов SDK.
fn sdk_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2019, 11, 21, 10, 0, 0).unwrap()
}

#[test]
fn signature_matches_sdk_signer() {
    // signer_test.go: Sign("GET", "api.huobi.pro", "/v1/account/history", "account-id=1&currency=btcusdt")
    let s = sign::signature(
        "GET",
        "api.huobi.pro",
        "/v1/account/history",
        "account-id=1&currency=btcusdt",
        "secret",
    )
    .unwrap();
    assert_eq!(s, "HUP3n78npIuTzVKyjEOrPictRKEUTRoYs7Ld5y38hmA=");
}

#[test]
fn signed_query_matches_sdk_without_params() {
    // privateurlbuilder_test.go: BuildWithTime("GET", "/v1/account/accounts", 2019-11-21T10:00:00, nil)
    let q = sign::signed_query(
        &sdk_keys(),
        "GET",
        "api.huobi.pro",
        "/v1/account/accounts",
        &[],
        sdk_time(),
    )
    .unwrap();
    assert_eq!(
        q,
        "AccessKeyId=access&SignatureMethod=HmacSHA256&SignatureVersion=2&Timestamp=2019-11-21T10%3A00%3A00&\
         Signature=rWnLcMt3XBAsmXoNHtTQVpvMbH%2FcE1PXFwQAGeYwt3s%3D"
    );
}

#[test]
fn signed_query_matches_sdk_with_params() {
    // privateurlbuilder_test.go: account-id=123, currency=btc на /v1/account/history
    let params = vec![
        ("currency", "btc".to_string()),
        ("account-id", "123".to_string()),
    ];
    let q = sign::signed_query(
        &sdk_keys(),
        "GET",
        "api.huobi.pro",
        "/v1/account/history",
        &params,
        sdk_time(),
    )
    .unwrap();
    assert_eq!(
        q,
        "AccessKeyId=access&SignatureMethod=HmacSHA256&SignatureVersion=2&Timestamp=2019-11-21T10%3A00%3A00&\
         account-id=123&currency=btc&Signature=SGZYJ9Ub%2FhFerEBbSWsCxl8Djk%2BLRBgEZOB4fLc4T9Q%3D"
    );
}

#[test]
fn ws_auth_matches_sdk() {
    // websocketv2requestbuilder_test.go: Init("access", "secret", "api.huobi.pro", "/ws/v2"), build(2019-11-21T10:00:00)
    let v = sign::ws_auth(&sdk_keys(), "api.huobi.pro", "/ws/v2", sdk_time()).unwrap();
    assert_eq!(v["action"], "req");
    assert_eq!(v["ch"], "auth");
    let p = &v["params"];
    assert_eq!(p["authType"], "api");
    assert_eq!(p["signatureVersion"], "2.1");
    assert_eq!(p["timestamp"], "2019-11-21T10:00:00");
    assert_eq!(
        p["signature"],
        "1/d+cUIEh4tC0aXho86zu5QAxVzJaTe56mUiB275T0E="
    );
    let (host, path) = sign::host_path("wss://API.huobi.pro/ws/v2").unwrap();
    assert_eq!((host.as_str(), path.as_str()), ("api.huobi.pro", "/ws/v2"));
}

#[test]
fn order_type_and_client_ids() {
    assert_eq!(
        order_rules::order_type(Side::Buy, "ioc").unwrap(),
        "buy-ioc"
    );
    assert_eq!(
        order_rules::order_type(Side::Sell, "fok").unwrap(),
        "sell-limit-fok"
    );
    assert!(order_rules::order_type(Side::Buy, "gtc").is_err());
    let long = "deal-2026-11-05-very-long-identifier-0001-and-even-longer-than-sixty-four";
    for (kind, max) in [
        ('b', order_rules::MAX_ORDER_CID),
        ('w', transfers::MAX_WITHDRAW_CID),
    ] {
        let id = order_rules::client_oid(kind, long, max);
        assert!(!id.is_empty() && id.len() <= max, "{id}");
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }
}

#[test]
#[ignore = "записать htx_market_symbols.json — GET /v1/settings/common/market-symbols (tests/README.md, раздел HTX)"]
fn recorded_symbols_cake() {
    let (map, skipped) = symbols::parse_list(&data("htx_market_symbols.json"), "USDT").unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    let r = &map["CAKE"];
    assert_eq!(r.symbol, "cakeusdt");
    assert!(r.tick_size > Decimal::ZERO && r.step_size > Decimal::ZERO);
    if r.tradable() {
        let (q, p) = order_rules::check_rules(r, Side::Buy, d("1000.123456789"), d("2.5")).unwrap();
        assert!(q <= d("1000.123456789") && p <= d("2.5"));
    }
}

#[test]
#[ignore = "записать htx_currencies_cake.json, htx_chains_cake.json, htx_bsc_network.txt, htx_contract_field.txt"]
fn recorded_coins_bsc() {
    let venue = VenueId("htx".into());
    let read = |f: &str| {
        std::fs::read_to_string(format!("{DIR}/{f}"))
            .expect(f)
            .trim()
            .to_string()
    };
    let (bsc, field) = (read("htx_bsc_network.txt"), read("htx_contract_field.txt"));
    let map = networks_parse::contracts(&data("htx_chains_cake.json"), &field).unwrap();
    let p = networks_parse::parse(&data("htx_currencies_cake.json"), &bsc, &map, &venue, false)
        .unwrap();
    let cake = p
        .list
        .iter()
        .find(|i| i.asset.ticker == "CAKE" && i.network == bsc)
        .expect("CAKE в сети BSC");
    let want: alloy_primitives::Address = CAKE.parse().unwrap();
    assert_eq!(cake.asset.bsc_contract, Some(want));
    assert!(p.chains.iter().any(|(t, n, _)| t == "CAKE" && *n == bsc));
}

#[test]
#[ignore = "записать htx_ws_mbp.jsonl — распакованные кадры wss://api.huobi.pro/ws, mbp.refresh.20 (tests/README.md)"]
fn recorded_ws_frames() {
    let t = std::fs::read_to_string(format!("{DIR}/htx_ws_mbp.jsonl")).unwrap();
    let (mut subscribed, mut books, mut pings) = (0, 0, 0);
    for line in t.lines().filter(|l| !l.trim().is_empty()) {
        match depth::parse(line) {
            depth::Msg::Subscribed(_) => subscribed += 1,
            depth::Msg::Ping(_) => pings += 1,
            depth::Msg::Depth { asks, bids, .. } => {
                books += 1;
                assert!(asks.windows(2).all(|w| w[0].price <= w[1].price));
                assert!(bids.windows(2).all(|w| w[0].price >= w[1].price));
            }
            depth::Msg::Bad { reason, .. } => panic!("кадр не разобран: {reason}: {line}"),
            _ => {}
        }
    }
    assert!(subscribed > 0 && books > 0 && pings > 0);
}

#[test]
#[ignore = "записать на живом боте htx_fee_rate.json — GET /v2/reference/transact-fee-rate?symbols=cakeusdt"]
fn recorded_fee_rate() {
    let m = fees::parse(&data("htx_fee_rate.json")).unwrap();
    assert!(m["cakeusdt"].taker > Decimal::ZERO);
}

#[test]
#[ignore = "записать на живом боте htx_matchresults.json — matchresults исполненного IOC-ордера"]
fn recorded_order_fee() {
    let fees = order_fee::parse(&data("htx_matchresults.json"), "CAKE").unwrap();
    assert!(
        fees.iter()
            .all(|f| f.amount > Decimal::ZERO && !f.asset.is_empty())
    );
}

#[test]
#[ignore = "записать на живом боте htx_deposits.json — GET /v1/query/deposit-withdraw?type=deposit"]
fn recorded_deposits() {
    let v = data("htx_deposits.json");
    let list = v.as_array().unwrap();
    for r in list {
        deposits::state(r).unwrap();
    }
    deposits::hold(list).unwrap();
}

#[test]
#[ignore = "записать на живом боте htx_accounts_update.jsonl — кадры accounts.update#2 (сырой журнал data/raw/htx)"]
fn recorded_balance_frames() {
    let t = std::fs::read_to_string(format!("{DIR}/htx_accounts_update.jsonl")).unwrap();
    let spot: i64 = std::fs::read_to_string(format!("{DIR}/htx_spot_account_id.txt"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let (venue, mut last) = (VenueId("htx".into()), account_update::Last::new());
    for line in t.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line).unwrap();
        if v["action"] == "push" {
            account_update::parse(&v["data"], spot, &mut last, &venue).unwrap();
        }
    }
    assert!(!last.is_empty());
}

/// Решения 202/213 для всех бирж (Тигран 08.11), venues/htx/book_print.rs — подтверждение тихой пары по содержимому.
/// Разбор tick ответа GET /market/depth на ПРИМЕРЕ ИЗ ДОКУМЕНТАЦИИ («Market Data → Get Market Depth», Response: tick.ts
/// 1629790438215, version 136107114472, первые два уровня bids [49790.87, 0.779876], [49785.9, 1.82E-4], asks
/// [49790.88, 2.980472], [49790.89, 0.006613] — в примере по 5 уровней, здесь взяты первые два) — живого ответа и
/// снимков mbp.refresh в tests/data нет (api.huobi.pro закрыт прокси; что записать — tests/README.md, раздел HTX).
/// Стакан в памяти — снимок mbp.refresh того же вида, что в документации (ch, ts, tick.seqNum, bids, asks), с уровнями
/// примера REST (собран здесь, не запись), разобранный depth.rs. Сравнение — живая логика ядра (core/feed_confirm.rs
/// verdict): тот же отпечаток — Quiet (1.82E-4 = 0.000182 — сравнение значений); стакана нет или он другой — Lost.
#[test]
fn book_print_docs_example() {
    use arb::core::feed_confirm::{Verdict, verdict};
    use arb::gateway::feed_health::{BookPrint, BookPrints, BookVersion, PairHealth};
    use arb::venues::htx::book_print;
    let tick: Value = serde_json::from_str(
        r#"{"ts":1629790438215,"version":136107114472,"bids":[[49790.87,0.779876],[49785.9,1.82E-4]],"asks":[[49790.88,2.980472],[49790.89,0.006613]]}"#,
    )
    .unwrap();
    let p = book_print::parse(&tick, 5).unwrap();
    assert_eq!((p.bids.len(), p.asks.len()), (2, 2));
    assert_eq!(
        (p.bids[0].price, p.bids[0].qty),
        (d("49790.87"), d("0.779876"))
    );
    assert_eq!(p.bids[1].qty, d("0.000182"));
    assert_eq!(
        (p.asks[0].price, p.asks[0].qty),
        (d("49790.88"), d("2.980472"))
    );
    let frame = r#"{"ch":"market.btcusdt.mbp.refresh.5","ts":1629790438215,"tick":{"seqNum":1,"bids":[[49790.87,0.779876],[49785.9,0.000182]],"asks":[[49790.88,2.980472],[49790.89,0.006613]]}}"#;
    let depth::Msg::Depth { bids, asks, .. } = depth::parse(frame) else {
        panic!("снимок mbp.refresh не разобран")
    };
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
    let other = BookPrint::top(&p.bids, &p.asks[..1], 5);
    assert!(matches!(
        verdict(&pair, &ver(Some(other)), 2),
        Verdict::Lost(_)
    ));
    // Вид ответа: tick нет (null), нет bids.
    assert!(book_print::parse(&Value::Null, 5).is_err());
    let bad: Value = serde_json::from_str(r#"{"asks":[]}"#).unwrap();
    assert_eq!(book_print::parse(&bad, 5).unwrap_err(), "нет bids");
}

/// Решение 202: отписка одной пары — вид из «Websocket Market Data → Unsubscribe» ({"unsub": тема, "id"}).
#[test]
fn unsubscribe_message_is_official_shape() {
    let v: Value =
        serde_json::from_str(&arb::venues::htx::ws_conn::unsub_message("btcusdt", 20)).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"unsub": "market.btcusdt.mbp.refresh.20", "id": "btcusdt"})
    );
}
