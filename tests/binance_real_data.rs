//! Файл: tests/binance_real_data.rs
//! Что делает: адаптер Binance (решение 160) — подпись на ОФИЦИАЛЬНЫХ примерах документации (выполняются всегда) и
//! разбор ответов на ЖИВЫХ записях tests/data/cex_samples_2026_11_05/binance_* (из среды разработки API Binance закрыт
//! прокси — записи делаются на сервере бота командами из tests/README.md, раздел «Binance»; до записи тесты
//! помечены #[ignore] с описанием, что записать).
//! - REST: request-security «SIGNED Endpoint Examples for POST /api/v3/order» (HMAC);
//! - WebSocket API: websocket-api/request-security «SIGNED request example (HMAC)».
//!
//! Кто вызывает: cargo test (записи — cargo test --test binance_real_data -- --ignored). От чего зависит:
//! arb::venues::binance. Моков нет.

use std::collections::HashSet;

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{Side, TapView, VenueId};
use arb::gateway::tap::{TapData, TapStream};
use arb::venues::binance::{
    account_update, all_balances, deposits, depth, fee_rates, key_check, networks_parse,
    order_rules, sign, symbols_parse, tap_decode,
};

const DIR: &str = "tests/data/cex_samples_2026_11_05";

/// Записанный ответ (JSON). Падает с понятным текстом, если записи нет.
fn data(file: &str) -> Value {
    let t = std::fs::read_to_string(format!("{DIR}/{file}")).unwrap_or_else(|_| {
        panic!("нет записи {DIR}/{file} — команды записи в tests/README.md, раздел Binance")
    });
    serde_json::from_str(&t).expect("JSON")
}

/// Строки записанного потока (jsonl).
fn lines(file: &str) -> Vec<String> {
    std::fs::read_to_string(format!("{DIR}/{file}"))
        .unwrap_or_else(|_| {
            panic!("нет записи {DIR}/{file} — команды записи в tests/README.md, раздел Binance")
        })
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn venue() -> VenueId {
    VenueId("binance".into())
}

#[test]
fn rest_signature_matches_official_example() {
    // request-security: секрет и строка из примера POST /api/v3/order (HMAC Keys).
    let p: Vec<(&str, String)> = [
        ("symbol", "LTCBTC"),
        ("side", "BUY"),
        ("type", "LIMIT"),
        ("timeInForce", "GTC"),
        ("quantity", "1"),
        ("price", "0.1"),
        ("recvWindow", "5000"),
        ("timestamp", "1499827319559"),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.to_string()))
    .collect();
    let q = sign::query(&p).unwrap();
    assert_eq!(
        q,
        "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559"
    );
    let secret = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
    assert_eq!(
        sign::hmac_hex(secret, &q).unwrap(),
        "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71"
    );
}

#[test]
fn ws_api_signature_matches_official_example() {
    // websocket-api/request-security: параметры даны не по алфавиту — ws_payload сортирует их сам.
    let p: Vec<(&str, String)> = [
        ("symbol", "BTCUSDT"),
        ("side", "SELL"),
        ("type", "LIMIT"),
        ("timeInForce", "GTC"),
        ("quantity", "0.01000000"),
        ("price", "52000.00"),
        ("recvWindow", "100"),
        ("timestamp", "1645423376532"),
        (
            "apiKey",
            "vmPUZE6mv9SD5VNHk4HlWFsOr6aKE2zvsw0MuIgwCIPy6utIco14y7Ju91duEh8A",
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.to_string()))
    .collect();
    let payload = sign::ws_payload(&p);
    assert_eq!(
        payload,
        "apiKey=vmPUZE6mv9SD5VNHk4HlWFsOr6aKE2zvsw0MuIgwCIPy6utIco14y7Ju91duEh8A&price=52000.00&quantity=0.01000000&recvWindow=100&side=SELL&symbol=BTCUSDT&timeInForce=GTC&timestamp=1645423376532&type=LIMIT"
    );
    let secret = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
    assert_eq!(
        sign::hmac_hex(secret, &payload).unwrap(),
        "aa1b5712c094bc4e57c05a1a5c1fd8d88dcd628338ea863fec7b88e59fe2db24"
    );
}

#[test]
#[ignore = "записать binance_exchange_info_CAKEUSDT.json (публичный) и binance_account.json (ключ) — tests/README.md"]
fn exchange_info_cake_rules_and_order_rules() {
    let perms: HashSet<String> =
        symbols_parse::account_permissions(&data("binance_account.json")).unwrap();
    let map =
        symbols_parse::parse_list(&data("binance_exchange_info_CAKEUSDT.json"), "USDT", &perms)
            .unwrap();
    let cake = &map["CAKE"];
    assert_eq!(cake.symbol, "CAKEUSDT");
    assert_eq!(cake.status_text, "TRADING");
    assert!(
        cake.tradable(true) && cake.tradable(false),
        "permissionSets не покрыты: {perms:?}"
    );
    assert!(cake.step_size > Decimal::ZERO && cake.tick_size > Decimal::ZERO);
    assert!(
        cake.order_types
            .as_ref()
            .unwrap()
            .iter()
            .any(|t| t == "LIMIT")
    );
    // Покупка — количество и цена вниз до шага; продажа — цена вверх; IOC проходит, GTC — нет.
    let price = cake.min_price.unwrap_or(cake.tick_size) * Decimal::from(1000);
    let qty = cake.min_notional.unwrap_or(Decimal::ONE) * Decimal::from(2) / price;
    let (q, p) = order_rules::check_rules(cake, Side::Buy, qty, price, "IOC").unwrap();
    assert!(q <= qty && p <= price);
    assert!(order_rules::check_rules(cake, Side::Buy, qty, price, "GTC").is_err());
}

#[test]
#[ignore = "записать binance_ws_depth20.jsonl (SUBSCRIBE cakeusdt@depth20@100ms на /stream) — tests/README.md"]
fn partial_depth_frames() {
    let mut books = 0;
    for l in lines("binance_ws_depth20.jsonl") {
        match depth::parse(&l) {
            depth::Msg::Depth { symbol, book } => {
                assert_eq!(symbol, "cakeusdt");
                let (bids, asks) = depth::levels(&book).unwrap();
                assert!(bids.windows(2).all(|w| w[0].price > w[1].price));
                assert!(asks.windows(2).all(|w| w[0].price < w[1].price));
                assert!(bids[0].price < asks[0].price);
                let t = tap_decode::decode(TapStream::Book, &TapData::Text(l.clone())).unwrap();
                assert!(matches!(t.view, TapView::Book { .. }));
                books += 1;
            }
            depth::Msg::Ack(id) => assert_eq!(id, "1"),
            depth::Msg::Error { code, msg, .. } => panic!("отказ {code}: {msg}"),
            _ => {}
        }
    }
    assert!(books > 5);
}

#[test]
#[ignore = "записать binance_ws_trade.jsonl, binance_ws_diff.jsonl (/ws/cakeusdt@trade, /ws/cakeusdt@depth@100ms) и binance_depth_CAKEUSDT_100.json — tests/README.md"]
fn window_trade_diff_snapshot() {
    let t = lines("binance_ws_trade.jsonl");
    let d = tap_decode::decode(TapStream::Trades, &TapData::Text(t[0].clone())).unwrap();
    let TapView::Trades(ticks) = d.view else {
        panic!("не сделки")
    };
    assert!(ticks[0].price > Decimal::ZERO && !ticks[0].id.is_empty());
    let diffs = lines("binance_ws_diff.jsonl");
    let ranges: Vec<(i64, i64)> = diffs
        .iter()
        .filter_map(|l| tap_decode::diff_range(l))
        .collect();
    assert!(
        ranges.windows(2).all(|w| w[1].0 == w[0].1 + 1),
        "разрыв U/u: {ranges:?}"
    );
    let snap = data("binance_depth_CAKEUSDT_100.json");
    assert!(tap_decode::snapshot_id(&snap).is_some());
    let s = tap_decode::decode(TapStream::Snapshot, &TapData::Text(snap.to_string())).unwrap();
    assert!(matches!(s.view, TapView::Diff { full: true, .. }));
}

#[test]
#[ignore = "записать binance_config_getall.json (ключ, GET /sapi/v1/capital/config/getall) — tests/README.md"]
fn config_getall_cake_bsc() {
    let min = networks_parse::DepositMin::default();
    let p =
        networks_parse::parse_networks(&data("binance_config_getall.json"), "BSC", &venue(), min)
            .unwrap();
    let cake = p
        .list
        .iter()
        .find(|i| i.asset.ticker == "CAKE" && i.network == "BSC")
        .expect("CAKE в сети BSC");
    // Адрес контракта CAKE в BSC (PancakeSwap Token) — сверка с пулом по адресу (урок 8).
    let want: alloy_primitives::Address = "0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82"
        .parse()
        .unwrap();
    assert_eq!(cake.asset.bsc_contract, Some(want));
    assert!(cake.withdraw_fee.is_some() && cake.withdraw_min.is_some());
    assert_eq!(
        cake.deposit_min, None,
        "минимум депозита — только по решению человека"
    );
    assert!(
        p.list
            .iter()
            .any(|i| i.asset.ticker == "USDT" && i.network == "BSC")
    );
}

#[test]
#[ignore = "записать binance_account_commission_CAKEUSDT.json (ключ, GET /api/v3/account/commission) — tests/README.md"]
fn commission_rates_and_plan() {
    let r = fee_rates::Rates::parse(&data("binance_account_commission_CAKEUSDT.json")).unwrap();
    let full = r.full(Side::Buy);
    assert!(full > Decimal::ZERO);
    let (rate, in_bnb, _) = r.plan(fee_rates::FEE_COIN, Side::Buy, true).unwrap();
    assert_eq!((rate, in_bnb), (full, false));
    let (rate, _, _) = r.plan(fee_rates::FEE_BNB, Side::Sell, false).unwrap();
    assert_eq!(
        rate,
        r.full(Side::Sell),
        "без монеты скидки — полная ставка"
    );
    assert!(r.plan("gt", Side::Buy, true).is_err());
}

#[test]
#[ignore = "записать binance_deposit_hisrec.json (ключ, GET /sapi/v1/capital/deposit/hisrec) — tests/README.md"]
fn deposit_history_hold() {
    let h = deposits::parse_hold(&data("binance_deposit_hisrec.json")).unwrap();
    for i in &h.items {
        assert!(i.amount > Decimal::ZERO && !i.asset.is_empty());
    }
}

#[test]
#[ignore = "записать binance_user_stream.jsonl (поток аккаунта WebSocket API с депозитом/выводом) — tests/README.md"]
fn user_stream_balance_update() {
    let mut seen = 0;
    for l in lines("binance_user_stream.jsonl") {
        if let account_update::Event::Balance(c) = account_update::parse(&l, &venue()) {
            assert!(!c.delta.is_zero());
            seen += 1;
        }
    }
    assert!(seen > 0, "в записи нет balanceUpdate");
}

#[test]
#[ignore = "записать binance_questionnaire.json, binance_funding_asset.json, binance_wallet_balance.json (ключ) — tests/README.md"]
fn key_check_and_balances_replies() {
    let q = data("binance_questionnaire.json");
    println!("travel rule NIL: {}", key_check::has_nil(&q));
    all_balances::parse_funding(&venue(), &data("binance_funding_asset.json")).unwrap();
    println!(
        "{}",
        all_balances::wallets_note(&data("binance_wallet_balance.json")).unwrap()
    );
}
