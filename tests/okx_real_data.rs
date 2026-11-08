//! Файл: tests/okx_real_data.rs
//! Что делает: адаптер OKX (решение 161) — подпись по формуле официального SDK github.com/okxapi/python-okx v0.4.4
//! (okx/utils.py signature, okx/websocket/WsUtils.py initLoginParams) и разбор ЖИВЫХ ответов OKX из
//! tests/data/cex_samples_2026_11_05/okx_* — их из среды разработки записать нельзя (www.okx.com закрыт прокси), поэтому
//! тесты на записях помечены #[ignore] с тем, что записать (команды — tests/README.md, раздел OKX).
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::okx. Моков нет.

use std::collections::HashSet;

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{Side, TransferStatus, VenueId};
use arb::venues::okx::depth::{self, Msg};
use arb::venues::okx::local_book::{Applied, LocalBook};
use arb::venues::okx::{
    deposit_addr, deposit_event, deposits, fees, networks_parse, order_rules, rest, sign, symbols,
    transfers,
};

const DIR: &str = "tests/data/cex_samples_2026_11_05";

/// data ответа REST из записи (code = "0"). Паникует, если файла нет — тест помечен #[ignore] до записи.
fn data(file: &str) -> Value {
    let t = std::fs::read_to_string(format!("{DIR}/{file}")).expect("файл записи");
    let mut v: Value = serde_json::from_str(&t).expect("JSON");
    assert_eq!(v["code"], "0", "{file}: code не 0");
    v.as_object_mut().unwrap().remove("data").unwrap()
}

/// Строки записи WebSocket (по кадру на строку).
fn frames(file: &str) -> Vec<String> {
    let t = std::fs::read_to_string(format!("{DIR}/{file}")).expect("файл записи");
    t.lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Подпись: Base64(HMAC-SHA256(timestamp + METHOD + requestPath + body)) — формула signature() SDK; значения ниже
/// посчитаны той же формулой (стандартный hmac/base64 Python) на примере документации «REST Authentication»
/// (GET /api/v5/account/balance?ccy=BTC, время 2020-12-08T09:08:57.715Z) и входе WebSocket (initLoginParams:
/// timestamp + 'GET' + '/users/self/verify'). Секрет — условный тестовый.
#[test]
fn signature_matches_sdk_formula() {
    let secret = b"22582BD0CFF14C41EDBF1AB98506286D";
    let ts = "2020-12-08T09:08:57.715Z";
    let path = rest::path_query("/api/v5/account/balance", &vec![("ccy", "BTC".into())]).unwrap();
    assert_eq!(path, "/api/v5/account/balance?ccy=BTC");
    let get = sign::prehash(ts, "get", &path, "");
    assert_eq!(
        get,
        "2020-12-08T09:08:57.715ZGET/api/v5/account/balance?ccy=BTC"
    );
    assert_eq!(
        sign::hmac_b64(get.as_bytes(), secret).unwrap(),
        "HiZhvSfMtWJA3uUIVXV3a/bSXNPCWvYFXoGCVS8V4zY="
    );
    let body = r#"{"instId":"BTC-USDT","tdMode":"cash","side":"buy","ordType":"ioc","px":"2.15","sz":"2"}"#;
    let post = sign::prehash(ts, "POST", "/api/v5/trade/order", body);
    assert_eq!(
        sign::hmac_b64(post.as_bytes(), secret).unwrap(),
        "mU//5P3rmWEG4O0JF1WAn9FkGApNHXWeaNJd5GVHg5o="
    );
    let ws = sign::prehash("1538054050", "GET", sign::WS_LOGIN_PATH, "");
    assert_eq!(
        sign::hmac_b64(ws.as_bytes(), secret).unwrap(),
        "+LdIr8lkkvhr5hoA3g9TMC0+uQJ849ftAcocA/ouu4M="
    );
    // Время — ISO 8601 UTC с миллисекундами, как пример документации.
    let at = chrono::DateTime::parse_from_rfc3339("2020-12-08T09:08:57.715Z").unwrap();
    assert_eq!(sign::rest_timestamp(at.into()), ts);
}

#[test]
#[ignore = "записать okx_instruments_spot.json — tests/README.md, раздел OKX"]
fn instruments_cake_rules() {
    let (map, skipped) = symbols::parse_list(&data("okx_instruments_spot.json"), "USDT").unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    let cake = &map["CAKE"];
    assert_eq!(cake.symbol, "CAKE-USDT");
    assert!(cake.tradable(), "{}", cake.state);
    // Покупка: количество вниз до lotSz, цена вниз до tickSz; продажа — цена вверх; GTC-подобный тип — отказ.
    let (q, p) =
        order_rules::check_rules(cake, Side::Buy, d("10.123456789"), d("1.89199"), "ioc").unwrap();
    assert!(q <= d("10.123456789") && p <= d("1.89199"));
    let (_, ps) = order_rules::check_rules(cake, Side::Sell, d("10"), d("1.89101"), "ioc").unwrap();
    assert!(ps >= d("1.89101"));
    assert!(order_rules::check_rules(cake, Side::Buy, d("10"), d("1.8"), "limit").is_err());
}

#[test]
#[ignore = "записать okx_books5.jsonl и okx_books.jsonl — tests/README.md, раздел OKX"]
fn ws_books_frames() {
    let mut n5 = 0;
    for f in frames("okx_books5.jsonl") {
        if let Msg::Depth(d) = depth::parse(&f) {
            assert!(d.snapshot && d.bids.len() <= 5 && d.asks.len() <= 5);
            n5 += 1;
        }
    }
    assert!(n5 > 0, "в записи books5 нет кадров стакана");
    // books: снимок и изменения подряд — без разрывов prevSeqId.
    let mut lb = LocalBook::default();
    let mut changed = 0;
    for f in frames("okx_books.jsonl") {
        if let Msg::Depth(d) = depth::parse(&f) {
            match lb.apply(&d) {
                Applied::Gap(why) => panic!("разрыв на живой записи: {why}"),
                Applied::Changed => changed += 1,
                Applied::Repeat => {}
            }
        }
    }
    assert!(changed > 1 && lb.version().is_some());
    let (bids, asks) = lb.top(400);
    assert!(bids[0].price < asks[0].price);
}

#[test]
#[ignore = "приватная запись на живом боте: okx_currencies_CAKE.json — tests/README.md, раздел OKX"]
fn currencies_cake_bsc() {
    let venue = VenueId("okx".into());
    let p = networks_parse::parse(&data("okx_currencies_CAKE.json"), "BSC", &venue, false).unwrap();
    assert!(p.skipped.is_empty(), "{:?}", p.skipped);
    let bsc = p
        .list
        .iter()
        .find(|i| i.network == "BSC")
        .expect("сеть BSC у CAKE");
    assert!(bsc.asset.bsc_contract.is_some(), "ctAddr не полный адрес");
    assert!(
        p.chains
            .iter()
            .any(|(t, n, c)| t == "CAKE" && n == "BSC" && c.starts_with("CAKE-"))
    );
}

#[test]
#[ignore = "приватная запись на живом боте: okx_deposit_address_CAKE.json — tests/README.md, раздел OKX"]
fn deposit_address_pick() {
    let a = deposit_addr::pick(&data("okx_deposit_address_CAKE.json"), "CAKE-BSC").unwrap();
    assert!(a.address.starts_with("0x") && (a.to == "6" || a.to == "18"));
}

#[test]
#[ignore = "приватные записи на живом боте: okx_deposit_history.json, okx_withdrawal_history.json — tests/README.md"]
fn deposit_and_withdrawal_history() {
    let dep = data("okx_deposit_history.json");
    for r in dep.as_array().unwrap() {
        let _ = deposits::state(r).unwrap();
    }
    let _ = deposits::hold(&dep).unwrap();
    let wd = data("okx_withdrawal_history.json");
    let first = &wd.as_array().unwrap()[0];
    assert!(matches!(
        transfers::status(first),
        Ok(TransferStatus::Sent | TransferStatus::Pending) | Err(_)
    ));
}

#[test]
#[ignore = "приватная запись на живом боте: okx_trade_fee_CAKE.json — tests/README.md, раздел OKX"]
fn trade_fee_sign() {
    let r =
        fees::parse(rest::first(&data("okx_trade_fee_CAKE.json"), "trade-fee").unwrap()).unwrap();
    // Комиссия taker у обычного уровня — положительная доля (у OKX приходит отрицательной).
    assert!(r.taker > Decimal::ZERO, "{r:?}");
}

#[test]
#[ignore = "приватная запись на живом боте: okx_deposit_info.jsonl (кадры канала deposit-info) — tests/README.md"]
fn deposit_info_frames() {
    let venue = VenueId("okx".into());
    let mut seen = HashSet::new();
    let mut credited = 0;
    for f in frames("okx_deposit_info.jsonl") {
        if let Msg::Data { channel, data } = depth::parse(&f)
            && channel == "deposit-info"
        {
            for row in data.as_array().unwrap() {
                let (_, _, c) = deposit_event::parse(row, &venue, &mut seen).unwrap();
                credited += usize::from(c.is_some_and(|c| c.tx_hash.is_some()));
            }
        }
    }
    assert!(
        credited > 0,
        "в записи нет зачисления (state 1 или 2) с txId"
    );
}

/// Решения 202/213 для всех бирж (Тигран 08.11), venues/okx/book_print.rs — подтверждение тихой пары по содержимому.
/// Разбор ответа GET /api/v5/market/books на ПРИМЕРЕ ИЗ ДОКУМЕНТАЦИИ («Order Book Trading → Market Data → GET / Order
/// book», Response Example: asks [["41006.8","0.60038921","0","1"]], bids [["41006.3","0.30178218","0","2"]],
/// ts "1629966436396", seqId 3235851742) — живого ответа REST и кадров books5 в tests/data нет (www.okx.com закрыт
/// прокси; что записать — tests/README.md, раздел OKX). Сравнение — живая логика ядра (core/feed_confirm.rs verdict):
/// тот же отпечаток в памяти — Quiet; стакана в памяти нет или он другой (нет стороны asks) — Lost.
#[test]
fn book_print_docs_example() {
    use arb::core::feed_confirm::{Verdict, verdict};
    use arb::gateway::feed_health::{BookPrint, BookPrints, BookVersion, PairHealth};
    use arb::venues::okx::book_print;
    let docs: Value = serde_json::from_str(
        r#"{"code":"0","msg":"","data":[{"asks":[["41006.8","0.60038921","0","1"]],"bids":[["41006.3","0.30178218","0","2"]],"ts":"1629966436396","seqId":3235851742}]}"#,
    )
    .unwrap();
    let p = book_print::parse(&docs["data"], 5).unwrap();
    assert_eq!((p.bids.len(), p.asks.len()), (1, 1));
    assert_eq!(
        (p.asks[0].price, p.asks[0].qty),
        (d("41006.8"), d("0.60038921"))
    );
    assert_eq!(
        (p.bids[0].price, p.bids[0].qty),
        (d("41006.3"), d("0.30178218"))
    );
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
    assert_eq!(verdict(&pair, &ver(Some(p.clone())), 2), Verdict::Quiet);
    assert!(matches!(verdict(&pair, &ver(None), 2), Verdict::Lost(_)));
    let other = BookPrint::top(&p.bids, &[], 5);
    assert!(matches!(
        verdict(&pair, &ver(Some(other)), 2),
        Verdict::Lost(_)
    ));
    // Вид ответа: data пустой, нет asks.
    let empty: Value = serde_json::from_str("[]").unwrap();
    assert!(book_print::parse(&empty, 5).is_err());
    let bad: Value = serde_json::from_str(r#"[{"bids":[],"ts":"1"}]"#).unwrap();
    assert_eq!(book_print::parse(&bad, 5).unwrap_err(), "нет asks");
}
