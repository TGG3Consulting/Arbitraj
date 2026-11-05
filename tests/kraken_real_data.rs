//! Файл: tests/kraken_real_data.rs
//! Что делает: адаптер Kraken (решение 172) — подпись на официальном проверочном примере гида [AUTH] (AddOrder,
//! nonce 1616492376594), подпись Funding (Beta) по формуле помощника гида [FUND] (значения посчитаны стандартными
//! hmac/hashlib/base64/urllib.parse.urlencode Python), CRC32 на стандартном проверочном значении, формат чек-суммы
//! стакана на примере гида book-checksum-v2, тексты ошибок для человека (решение 164) — и разбор ЖИВЫХ ответов Kraken из
//! tests/data/cex_samples_2026_11_05/kraken_* — их из среды разработки записать нельзя (docs.kraken.com и
//! api.kraken.com закрыты прокси), поэтому тесты на записях помечены #[ignore] с тем, что записать (команды —
//! tests/README.md, раздел Kraken).
//!
//! Кто вызывает: cargo test. От чего зависит: arb::venues::kraken, arb::notify::reason. Моков нет.

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{Side, VenueId};
use arb::notify::reason;
use arb::venues::kraken::depth::{self, Msg};
use arb::venues::kraken::local_book::{Applied, LocalBook};
use arb::venues::kraken::{
    account, balance_event, checksum, deposit_addr, deposits, errors, fees, methods,
    networks_parse, order_rules, order_state, reply, rest, sign, symbols, withdraw_addr,
};

const DIR: &str = "tests/data/cex_samples_2026_11_05";
/// Приватный ключ из проверочного примера гида [AUTH] (не настоящий).
const DOC_SECRET: &str =
    "kQH5HW/8p1uGOVjbgWA7FunAmGO8lsSUXNsu3eow76sz84Q18fWxnyRzBHCd3pd5nE9qa99HAZtuZuj6F1huXg==";

/// Тело записи как JSON. Паникует, если файла нет — тест помечен #[ignore] до записи.
fn json(file: &str) -> Value {
    let t = std::fs::read_to_string(format!("{DIR}/{file}")).expect("файл записи");
    serde_json::from_str(&t).expect("JSON")
}

/// result ответа спот REST из записи (error пустой).
fn result(file: &str) -> Value {
    let mut v = json(file);
    assert_eq!(v["error"], serde_json::json!([]), "{file}: error не пустой");
    v.as_object_mut().unwrap().remove("result").unwrap()
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

/// Официальный пример гида [AUTH]: API-Sign AddOrder — совпадает с документацией.
#[test]
fn spot_signature_matches_official_example() {
    let body = "nonce=1616492376594&ordertype=limit&pair=XBTUSD&price=37500&type=buy&volume=1.25";
    let s = sign::api_sign(
        "/0/private/AddOrder",
        "1616492376594",
        body.as_bytes(),
        DOC_SECRET,
    )
    .unwrap();
    assert_eq!(
        s,
        "4/dpxb3iT4tp/ZCVEwSnEsLxx0bqyhLpdfOpc6fn7OR8+UClSV5n9E6aSS8MPtnRfp32bAb0nmbRn6H8ndwLUQ=="
    );
    // Тело формы собирается так же, как в примере (порядок полей сохраняется).
    let form = vec![
        rest::p("nonce", "1616492376594"),
        rest::p("ordertype", "limit"),
        rest::p("pair", "XBTUSD"),
        rest::p("price", "37500"),
        rest::p("type", "buy"),
        rest::p("volume", "1.25"),
    ];
    assert_eq!(rest::urlencode(&form), body);
}

/// Funding (Beta): строка запроса как urlencode Python, подпись по формуле помощника гида [FUND].
#[test]
fn funding_signature_matches_helper_formula() {
    let q = vec![
        rest::p("asset[class]", "currency"),
        rest::p("asset[name]", "USDT"),
    ];
    let qs = rest::urlencode(&q);
    assert_eq!(qs, "asset%5Bclass%5D=currency&asset%5Bname%5D=USDT");
    let path = format!("/funding/v1/methods/withdraw?{qs}");
    assert_eq!(
        sign::api_sign(&path, "1616492376595", b"", DOC_SECRET).unwrap(),
        "zYX+riCSZkE4sOPGu+RyTwoEVr6krOrUCoHGZ+CRByq420wDoXSHcjouQ+ZeBW0SYWf6r/1+SWV1zsJ119/8PA=="
    );
    let body = serde_json::json!({"method_id": "27ede8db-804b-4d91-8e25-46b7b9668730"}).to_string();
    assert_eq!(
        body,
        r#"{"method_id":"27ede8db-804b-4d91-8e25-46b7b9668730"}"#
    );
    assert_eq!(
        sign::api_sign(
            "/funding/v1/deposit/address",
            "1616492376596",
            body.as_bytes(),
            DOC_SECRET
        )
        .unwrap(),
        "xlDSpELoMbZZFT/8tbVTyRMst//F7mbpu2K3rs7U1dnznd0/APP0ymi8gyB/mbdXgNsmTPxOJWIJ6II29YyiLw=="
    );
    // nonce помощника: max(время, прежний + 1).
    assert_eq!(sign::next_nonce(10, 5), 11);
    assert_eq!(sign::next_nonce(10, 50), 50);
    assert!(sign::api_sign("/x", "1", b"", "не base64!").is_err());
}

/// CRC32 IEEE — стандартное проверочное значение «123456789» → 0xCBF43926; формат уровня — пример гида чек-суммы.
#[test]
fn checksum_crc32_and_level_format() {
    assert_eq!(checksum::crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(checksum::field(d("45285.2"), 1), "452852");
    assert_eq!(checksum::field(d("0.001"), 8), "100000");
    assert_eq!(checksum::field(d("0.00100000"), 8), "100000");
}

/// Тексты ошибок Kraken для человека (решение 164) и паузы «слишком часто».
#[test]
fn errors_are_human_and_paused() {
    let list = vec!["EOrder:Insufficient funds".to_string()];
    let e = errors::from_errors(&list, errors::Op::Trade, "");
    assert_eq!(e.code, arb::gateway::errors::ErrorCode::InsufficientBalance);
    let r = reason::human(&format!("продажа не состоялась: {}", e.message));
    assert_eq!(r.text, "Продажа не состоялась — Kraken: не хватает средств");
    let e = errors::from_errors(
        &["EGeneral:Invalid arguments:volume minimum not met".into()],
        errors::Op::Trade,
        "",
    );
    assert_eq!(e.code, arb::gateway::errors::ErrorCode::OrderRejected);
    assert!(
        reason::human(&e.message)
            .text
            .contains("Kraken не принял параметры")
    );
    assert!(errors::is_rate_limit("EService: Throttled: 1700000000"));
    assert_eq!(
        reply::throttled_until("EService: Throttled: 1700000000"),
        Some(1_700_000_000)
    );
    let unknown = reason::human("Kraken, ошибка EFoo:Bar baz");
    assert!(unknown.answer.is_some());
}

/// Свой id ордера — новый на попытку, только ASCII, не длиннее 18.
#[test]
fn client_order_id_fits_doc_limit() {
    let id = order_rules::cl_ord_id(true, "20261105CAKE-deal-123456");
    assert!(id.len() <= 18 && id.is_ascii() && id.starts_with('b'));
}

#[test]
#[ignore = "записать kraken_asset_pairs.json (tests/README.md, раздел Kraken)"]
fn asset_pairs_cake_usdt() {
    let (map, skipped) = symbols::parse_list(&result("kraken_asset_pairs.json"), "USDT").unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    let r = &map["CAKE"];
    assert!(r.tradable() && r.tick_size > Decimal::ZERO, "{r:?}");
    let ok = order_rules::check_rules(r, Side::Sell, d("100"), d("2.5"), "IOC");
    assert!(ok.is_ok(), "{ok:?}");
}

#[test]
#[ignore = "записать kraken_methods_deposit.json, kraken_methods_withdraw.json, kraken_assets.json (ключ Query)"]
fn funding_methods_bsc() {
    let dep = json("kraken_methods_deposit.json")["methods"]
        .as_array()
        .unwrap()
        .clone();
    let wd = json("kraken_methods_withdraw.json")["methods"]
        .as_array()
        .unwrap()
        .clone();
    let assets = methods::parse_assets(&result("kraken_assets.json"));
    let bsc = std::env::var("KRAKEN_BSC").expect("имя сети BSC из записи (network_name)");
    let p =
        networks_parse::parse(&dep, &wd, &assets, &bsc, &VenueId("kraken".into()), false).unwrap();
    let cake = p
        .list
        .iter()
        .find(|i| i.asset.ticker == "CAKE" && i.network == bsc)
        .expect("CAKE на BSC");
    assert!(
        cake.asset.bsc_contract.is_some(),
        "contract_address у CAKE/BSC"
    );
}

#[test]
#[ignore = "записать kraken_book.jsonl (WebSocket v2 book CAKE/USDT depth 10) и kraken_asset_pairs.json"]
fn book_frames_checksum() {
    let (map, _) = symbols::parse_list(&result("kraken_asset_pairs.json"), "USDT").unwrap();
    let r = &map["CAKE"];
    let mut lb = LocalBook::new(10, r.price_decimals, r.lot_decimals);
    let mut changed = 0;
    for f in frames("kraken_book.jsonl") {
        if let Msg::Book(rows) = depth::parse(&f) {
            for row in rows {
                assert_eq!(lb.apply(&row.unwrap()), Applied::Changed);
                changed += 1;
            }
        }
    }
    assert!(changed > 1);
}

#[test]
#[ignore = "записать приватные kraken_trade_volume.json, kraken_balance_ex.json, kraken_api_key_info.json"]
fn private_spot_answers() {
    let rates = fees::parse(&result("kraken_trade_volume.json")).unwrap();
    assert!(
        rates
            .values()
            .all(|r| r.taker > Decimal::ZERO && r.taker < Decimal::ONE)
    );
    let (spot, _other) =
        account::parse(&VenueId("kraken".into()), &result("kraken_balance_ex.json")).unwrap();
    assert!(spot.iter().all(|b| b.free >= Decimal::ZERO));
    let k = result("kraken_api_key_info.json");
    assert!(
        k.get("permissions").is_some(),
        "имена полей GetApiKeyInfo: {k}"
    );
}

#[test]
#[ignore = "записать kraken_deposit_status.json, kraken_deposits.json, kraken_withdrawals.json, kraken_addresses.json, \
            kraken_deposit_addresses.json, kraken_fees_quote.json, kraken_query_orders.json, kraken_balances.jsonl"]
fn private_funding_answers() {
    deposits::hold(&result("kraken_deposit_status.json")).unwrap();
    let dep = json("kraken_deposits.json");
    for r in dep["deposits"].as_array().unwrap() {
        deposits::state(r).unwrap();
    }
    let wd = json("kraken_withdrawals.json");
    assert!(wd["withdrawals"].is_array());
    let addr = std::env::var("KRAKEN_WD_ADDR").expect("адрес из книги");
    withdraw_addr::find(&json("kraken_addresses.json"), &addr).unwrap();
    assert!(
        deposit_addr::pick(&json("kraken_deposit_addresses.json"))
            .unwrap()
            .is_some()
    );
    withdraw_addr::parse_quote(&json("kraken_fees_quote.json")).unwrap();
    let q = result("kraken_query_orders.json");
    let first = q.as_object().unwrap().values().next().unwrap().clone();
    let cl = first["cl_ord_id"]
        .as_str()
        .expect("cl_ord_id в ответе QueryOrders");
    assert!(order_state::by_client(Some(&q), cl).is_some());
    let mut seen = HashSet::new();
    let mut kinds: HashMap<String, usize> = HashMap::new();
    for f in frames("kraken_balances.jsonl") {
        if let Msg::Balances { data, .. } = depth::parse(&f) {
            for row in data.as_array().unwrap() {
                let (k, _, _) =
                    balance_event::parse(row, &VenueId("kraken".into()), &mut seen).unwrap();
                *kinds.entry(k).or_default() += 1;
            }
        }
    }
    assert!(!kinds.is_empty());
}
