//! Файл: tests/xt_real_data.rs
//! Что делает: адаптер XT.com (решение 167) — сборка строки подписи на ОФИЦИАЛЬНОМ примере документации и HMAC SHA256
//! на векторе RFC 4231 (выполняются всегда; подписи из примеров XT воспроизвести нельзя — секрет второго примера не
//! опубликован, тело первого дано не JSON-строкой), человеческая причина кода XT (решение 164), и разбор ответов на
//! ЖИВЫХ записях tests/data/cex_samples_2026_11_05/xt_* (из среды разработки sapi.xt.com закрыт прокси — записи
//! делаются на сервере бота командами из tests/README.md, раздел «XT»; до записи тесты помечены #[ignore]).
//!
//! Кто вызывает: cargo test (записи — cargo test --test xt_real_data -- --ignored). От чего зависит: arb::venues::xt,
//! arb::notify. Моков нет.

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::VenueId;
use arb::venues::xt::depth::{self, Msg};
use arb::venues::xt::local_book::{Applied, LocalBook, Update};
use arb::venues::xt::{all_balances, networks_parse, order_state, sign, symbols_parse};

const DIR: &str = "tests/data/cex_samples_2026_11_05";

/// Записанный ответ (JSON целиком, {rc, mc, ma, result}) → result. Падает с понятным текстом, если записи нет.
fn result(file: &str) -> Value {
    let t = std::fs::read_to_string(format!("{DIR}/{file}")).unwrap_or_else(|_| {
        panic!("нет записи {DIR}/{file} — команды записи в tests/README.md, раздел XT")
    });
    let v: Value = serde_json::from_str(&t).expect("JSON");
    v.get("result").cloned().unwrap_or(v)
}

/// Строки записанного потока (jsonl).
fn lines(file: &str) -> Vec<String> {
    std::fs::read_to_string(format!("{DIR}/{file}"))
        .unwrap_or_else(|_| panic!("нет записи {DIR}/{file} — tests/README.md, раздел XT"))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn venue() -> VenueId {
    VenueId("xt".into())
}

#[test]
fn signature_string_matches_official_example() {
    // signSteps.md, «4.example — sample of original signature message».
    let x = sign::spot_headers("2063495b-85ec-41b3-a810-be84ceb78751", 60000, 1666026215729);
    let body = r#"{"symbol":"XT_USDT","side":"BUY","type":"LIMIT","timeInForce":"GTC","bizType":"SPOT","price":3,"quantity":2}"#;
    let y = sign::data("POST", "/v4/order", "", body);
    assert_eq!(
        format!("{x}{y}"),
        r#"validate-algorithms=HmacSHA256&validate-appkey=2063495b-85ec-41b3-a810-be84ceb78751&validate-recvwindow=60000&validate-timestamp=1666026215729#POST#/v4/order#{"symbol":"XT_USDT","side":"BUY","type":"LIMIT","timeInForce":"GTC","bizType":"SPOT","price":3,"quantity":2}"#
    );
    // «query has data, body has no data: Y=#method#path#query» — ключи по алфавиту.
    let q = sign::query(&[("symbol", "btc_usdt".into()), ("limit", "500".into())]).unwrap();
    assert_eq!(
        sign::data("GET", "/v4/public/depth", &q, ""),
        "#GET#/v4/public/depth#limit=500&symbol=btc_usdt"
    );
    // Фьючерсы (_futures_documentation/signSteps.md): X без алгоритма и окна, Y без метода.
    assert_eq!(
        format!(
            "{}{}",
            sign::futures_headers("3976eb88-76d0-4f6e-a6b2-a57980770085", 1641446237201),
            sign::data("", "/future/user/v1/balance/list", "", "")
        ),
        "validate-appkey=3976eb88-76d0-4f6e-a6b2-a57980770085&validate-timestamp=1641446237201#/future/user/v1/balance/list"
    );
    assert!(sign::query(&[("a", "x y".into())]).is_err());
}

#[test]
fn hmac_is_sha256_hex_lowercase() {
    // RFC 4231, Test Case 2 — hmacSha256Hex из документации XT = HMAC-SHA-256, hex.
    assert_eq!(
        sign::hmac_hex("Jefe", "what do ya want for nothing?").unwrap(),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    assert!(sign::hmac_hex("", "x").is_err());
}

#[test]
fn xt_code_gets_human_reason() {
    let e = arb::venues::xt::errors::from_code(
        "AUTH_103",
        "[]",
        arb::venues::xt::errors::Op::Other,
        "",
    );
    let r = arb::notify::reason::human(&e.message);
    assert_eq!(r.text, "XT: ключ API или подпись не приняты");
    assert!(r.answer.is_none());
}

#[test]
#[ignore = "нужна запись xt_symbol_cake_usdt.json (GET /v4/public/symbol?symbol=cake_usdt)"]
fn symbol_rules_from_live_reply() {
    let p = symbols_parse::parse_list(&result("xt_symbol_cake_usdt.json"), "USDT").unwrap();
    let map = p.map.expect("список symbols");
    let cake = map.get("CAKE").expect("пара CAKE");
    assert_eq!(cake.symbol.to_lowercase(), "cake_usdt");
    assert!(cake.taker_fee.is_some(), "takerFeeRate пары");
    assert!(cake.qty.step().is_some() && cake.price.step().is_some());
    assert!(p.version.is_some());
}

#[test]
#[ignore = "нужны записи xt_support_currency.json и xt_currencies.json; venue.bsc_network — из записи (ARB_XT_BSC)"]
fn cake_bsc_contract_from_live_reply() {
    let bsc = std::env::var("ARB_XT_BSC").expect("ARB_XT_BSC = название сети BSC из записи");
    let st = networks_parse::parse_statuses(&result("xt_currencies.json")).unwrap();
    let p = networks_parse::parse_networks(
        &result("xt_support_currency.json"),
        &st,
        &bsc,
        &venue(),
        false,
    )
    .unwrap();
    let cake = p
        .list
        .iter()
        .find(|i| i.asset.ticker == "CAKE" && i.network == bsc)
        .expect("CAKE в сети BSC");
    let want: alloy::primitives::Address = "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82"
        .parse()
        .unwrap();
    assert_eq!(cake.asset.bsc_contract, Some(want));
    assert!(cake.withdraw_fee.is_some());
}

#[test]
#[ignore = "нужны записи xt_depth_cake_usdt.json (REST, limit=500) и xt_ws_depth_update.jsonl (WebSocket сразу после)"]
fn local_book_from_live_frames() {
    let snap = result("xt_depth_cake_usdt.json");
    let d: depth::RestDepth<'_> = serde::Deserialize::deserialize(&snap).unwrap();
    let mut lb = LocalBook::default();
    for l in lines("xt_ws_depth_update.jsonl") {
        if let Msg::Update { data, .. } = depth::parse(&l) {
            let u = Update {
                first: data.fi.expect("fi"),
                last: data.i,
                bids: depth::convert(&data.b, "b").unwrap(),
                asks: depth::convert(&data.a, "a").unwrap(),
            };
            assert_ne!(
                lb.apply(u),
                Applied::Changed,
                "до снимка изменения откладываются"
            );
        }
    }
    let applied = lb.snapshot(
        d.last_update_id,
        &depth::convert(&d.bids, "b").unwrap(),
        &depth::convert(&d.asks, "a").unwrap(),
    );
    assert_eq!(
        applied,
        Applied::Changed,
        "отложенные изменения продолжают снимок"
    );
    let (bids, asks) = lb.top(5);
    assert!(bids[0].price < asks[0].price);
}

#[test]
#[ignore = "нужна запись xt_ws_depth.jsonl (depth@cake_usdt,20)"]
fn depth_snapshots_from_live_frames() {
    let mut ids: HashMap<i64, usize> = HashMap::new();
    for l in lines("xt_ws_depth.jsonl") {
        if let Msg::Snapshot { data, .. } = depth::parse(&l) {
            let (b, a) = depth::snapshot_levels(&data).unwrap();
            assert!(b[0].price < a[0].price);
            *ids.entry(data.i).or_default() += 1;
        }
    }
    assert!(!ids.is_empty());
}

#[test]
#[ignore = "нужна запись xt_order.json (GET /v4/order/{id} исполненного IOC-ордера, ключ)"]
fn order_fee_from_live_reply() {
    let s = result("xt_order.json");
    assert!(order_state::is_final(&s));
    let (fee, asset) = order_state::fee(&s).unwrap();
    assert!(fee >= Decimal::ZERO && !asset.is_empty());
}

#[test]
#[ignore = "нужна запись xt_futures_balance.json (GET /future/user/v1/balance/list на fapi.xt.com, ключ)"]
fn futures_balances_from_live_reply() {
    all_balances::parse_futures(&venue(), &result("xt_futures_balance.json")).unwrap();
}
