//! Файл: tests/gate_real_data.rs
//! Что делает: разбор ответов Gate (решение 135) на ЖИВЫХ публичных записях 2026-11-04 (сервер, curl/websockets как
//! есть; tests/data/cex_samples_2026_11_04/gate_*): справочник пар, справочник монет и сетей (сеть BSC — «BSC»),
//! сети одной монеты (CAKE), REST-снимок стакана, кадры WebSocket (подписка, spot.pong, spot.order_book,
//! spot.order_book_update, spot.trades) и правила ордера на правилах живой пары. Приватные ответы (ключ) — тесты
//! #[ignore] с описанием, что записать на живом боте (tests/README.md).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::gate. Моков нет.

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{Side, TapView, VenueId};
use arb::gateway::tap::{TapData, TapStream};
use arb::venues::gate::depth::{self, Msg};
use arb::venues::gate::networks_parse::{self, Extra};
use arb::venues::gate::{order_rules, symbols, tap, tap_snap};

const DIR: &str = "tests/data/cex_samples_2026_11_04";

fn read(name: &str) -> String {
    std::fs::read_to_string(format!("{DIR}/{name}")).expect("файл записи")
}

fn json(name: &str) -> Value {
    serde_json::from_str(&read(name)).expect("JSON")
}

/// Кадры WebSocket записи: строки {"t", "msg"} → текст кадра как пришёл (строки {"sent"} пропускаются).
fn ws_frames() -> Vec<String> {
    read("gate_ws.jsonl")
        .lines()
        .filter_map(|l| {
            serde_json::from_str::<Value>(l)
                .ok()?
                .get("msg")?
                .as_str()
                .map(str::to_string)
        })
        .collect()
}

#[test]
fn currency_pairs_parse_all_usdt() {
    let map = symbols::parse_list(
        &json("gate_spot_currency_pairs.json"),
        "USDT",
        &HashSet::new(),
        1_793_740_713,
    )
    .expect("весь справочник разбирается");
    assert!(map.len() > 1500, "пар с USDT: {}", map.len());
    let cake = &map["CAKE"];
    assert_eq!(cake.symbol, "CAKE_USDT");
    assert_eq!(cake.status, symbols::TRADABLE);
    assert!(cake.tradable(true) && cake.tradable(false));
    // amount_precision 2 → шаг количества 0.01; precision 4 → шаг цены 0.0001.
    assert_eq!(cake.step_size, Decimal::new(1, 2));
    assert_eq!(cake.tick_size, Decimal::new(1, 4));
    assert_eq!(cake.min_qty, Some(Decimal::new(1, 2)));
    assert_eq!(cake.min_notional, Some(Decimal::from(3)));
    assert_eq!(cake.max_notional, Some(Decimal::from(5_000_000)));
    // max_base_amount в живом ответе нет — предела нет.
    assert_eq!(cake.max_qty, None);
}

#[test]
fn whitelist_closes_other_pairs() {
    let allowed: HashSet<String> = ["CAKE_USDT".to_string()].into();
    let map = symbols::parse_list(
        &json("gate_spot_currency_pairs.json"),
        "USDT",
        &allowed,
        1_793_740_713,
    )
    .expect("разбор");
    assert!(map["CAKE"].tradable(true));
    assert!(!map["POOL"].tradable(true) && !map["POOL"].tradable(false));
    // Статус по-прежнему «торгуется» — стакан нужен (живая цена), API закрыт.
    assert!(map["POOL"].online());
}

#[test]
fn not_started_side_is_closed() {
    // BTW_USDT: buy_start 1787554800 — до этого момента покупка через API закрыта.
    let map = symbols::parse_list(
        &json("gate_spot_currency_pairs.json"),
        "USDT",
        &HashSet::new(),
        1_787_554_799,
    )
    .expect("разбор");
    assert!(!map["BTW"].tradable(true));
    assert!(map["BTW"].tradable(false));
}

#[test]
fn currencies_bsc_networks() {
    // withdraw_status — приватный ответ, записи нет: комиссии и пределы пустые (сделки по ним нет — None).
    let p = networks_parse::parse_networks(
        &json("gate_spot_currencies.json"),
        &Value::Array(vec![]),
        "BSC",
        &VenueId("gate".into()),
        false,
        &HashMap::new(),
        &|_| Some(2),
    )
    .expect("весь справочник монет разбирается");
    let cake = p
        .list
        .iter()
        .find(|n| n.asset.ticker == "CAKE" && n.network == "BSC")
        .expect("CAKE в BSC");
    assert_eq!(
        cake.asset
            .bsc_contract
            .map(|a| a.to_string().to_lowercase()),
        Some("0x0e09fabb73bd3ade0a17ecc321fd13a19e81ce82".into())
    );
    assert!(cake.deposit_enabled && cake.withdraw_enabled);
    assert_eq!(cake.withdraw_fee, None);
    assert_eq!(cake.withdraw_precision, Some(2));
    // USDT: адрес BSC в смешанном регистре разбирается; сеть ETH адреса BSC не получает.
    let usdt = p
        .list
        .iter()
        .find(|n| n.asset.ticker == "USDT" && n.network == "BSC")
        .expect("USDT в BSC");
    assert!(usdt.asset.bsc_contract.is_some());
    let eth = p
        .list
        .iter()
        .find(|n| n.asset.ticker == "USDT" && n.network == "ETH")
        .expect("USDT в ETH");
    assert!(eth.asset.bsc_contract.is_none());
    // Сеть 0G USDT закрыта на ввод и вывод.
    let og = p
        .list
        .iter()
        .find(|n| n.asset.ticker == "USDT" && n.network == "0G")
        .expect("0G");
    assert!(!og.deposit_enabled && !og.withdraw_enabled);
}

#[test]
fn currency_chains_cake_decimal() {
    let (ex, tagged) = networks_parse::chains_extra(
        &json("gate_wallet_currency_chains_CAKE.json"),
        Extra::default(),
    )
    .expect("разбор");
    assert_eq!(ex.decimals.get("BSC"), Some(&8));
    assert!(tagged.is_empty(), "у CAKE в BSC is_tag = 0");
    // Точность сети сильнее точности пары.
    let extra: HashMap<String, Extra> = [("CAKE".to_string(), ex)].into();
    let one = json("gate_spot_currencies.json")
        .as_array()
        .and_then(|a| a.iter().find(|c| c["currency"] == "CAKE").cloned())
        .expect("CAKE");
    let p = networks_parse::parse_networks(
        &one,
        &Value::Array(vec![]),
        "BSC",
        &VenueId("gate".into()),
        false,
        &extra,
        &|_| Some(2),
    )
    .expect("одна монета");
    assert_eq!(p.list.len(), 1);
    assert_eq!(p.list[0].withdraw_precision, Some(8));
}

#[test]
fn rest_snapshot_for_window() {
    let d =
        tap_snap::decode_snapshot(&read("gate_spot_order_book_CAKE_USDT.json")).expect("снимок");
    let TapView::Diff {
        bids,
        asks,
        full,
        from,
        ..
    } = d.view
    else {
        panic!("не Diff")
    };
    assert!(full);
    assert_eq!(from, Some(2_065_088_898));
    assert_eq!((bids.len(), asks.len()), (20, 20));
}

#[test]
fn ws_frames_parse() {
    let frames = ws_frames();
    let (mut acks, mut pongs, mut books, mut ids) = (0, 0, 0, vec![]);
    for f in &frames {
        match depth::parse(f) {
            Msg::Ack { error, .. } => {
                assert!(error.is_none(), "подписка принята: {f}");
                acks += 1;
            }
            Msg::Pong => pongs += 1,
            Msg::Depth {
                symbol,
                update_id,
                bids,
                asks,
                ..
            } => {
                assert_eq!(symbol, "CAKE_USDT");
                assert_eq!((bids.len(), asks.len()), (20, 20));
                assert!(bids[0].price < asks[0].price);
                assert!(bids.windows(2).all(|w| w[0].price > w[1].price));
                ids.push(update_id);
                books += 1;
            }
            Msg::Bad { reason, .. } => panic!("кадр не разобран: {reason}"),
            _ => {}
        }
    }
    assert_eq!((acks, pongs), (3, 1));
    assert!(books > 50);
    assert!(
        ids.windows(2).all(|w| w[0] <= w[1]),
        "lastUpdateId не убывает"
    );
}

#[test]
fn ws_diff_and_trades_for_window() {
    let mut ver = tap_snap::Versions::default();
    let (mut diffs, mut trades) = (0, 0);
    for f in ws_frames() {
        if f.contains("\"spot.order_book_update\",\"event\":\"update\"") {
            let d = tap::decode(TapStream::Diff, &TapData::Text(f.clone())).expect("изменения");
            let TapView::Diff { from, to, .. } = d.view else {
                panic!("не Diff")
            };
            assert!(from <= to);
            // Первый кадр сессии — снимок нужен.
            if diffs == 0 {
                assert!(ver.need_snapshot(&f));
            }
            diffs += 1;
        }
        if f.contains("\"spot.trades\",\"event\":\"update\"") {
            let d = tap::decode(TapStream::Trades, &TapData::Text(f)).expect("сделка");
            let TapView::Trades(t) = d.view else {
                panic!("не Trades")
            };
            assert_eq!(t.len(), 1);
            assert!(t[0].taker == "buy" || t[0].taker == "sell");
            assert!(t[0].time_ms > 1_793_740_000_000);
            trades += 1;
        }
    }
    assert!(
        diffs > 10 && trades > 0,
        "изменений {diffs}, сделок {trades}"
    );
}

#[test]
fn order_rules_on_live_pair() {
    let map = symbols::parse_list(
        &json("gate_spot_currency_pairs.json"),
        "USDT",
        &HashSet::new(),
        1_793_740_713,
    )
    .expect("разбор");
    let cake = &map["CAKE"];
    let (q, p) = order_rules::check_rules(
        cake,
        Side::Buy,
        Decimal::new(123_456, 4),
        Decimal::new(189_489, 5),
        "ioc",
    )
    .expect("ioc");
    assert_eq!((q, p), (Decimal::new(1234, 2), Decimal::new(18948, 4)));
    let (_, p) = order_rules::check_rules(
        cake,
        Side::Sell,
        Decimal::from(5),
        Decimal::new(189_481, 5),
        "ioc",
    )
    .expect("продажа");
    assert_eq!(p, Decimal::new(18949, 4));
    assert!(
        order_rules::check_rules(cake, Side::Buy, Decimal::from(5), Decimal::ONE, "gtc").is_err(),
        "gtc остаток не снимает"
    );
    assert!(
        order_rules::check_rules(cake, Side::Buy, Decimal::ONE, Decimal::ONE, "ioc").is_err(),
        "сумма меньше 3 USDT"
    );
    let t = order_rules::text(true, "20261104120000000ABC");
    assert!(t.starts_with("t-") && t.len() <= 30);
    assert!(
        t[2..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
    );
}

#[test]
#[ignore = "записать на живом боте: GET /wallet/withdraw_status (весь и ?currency=CAKE) — tests/README.md"]
fn withdraw_status_fees() {
    // Записать ответ GET /wallet/withdraw_status: проверить withdraw_fix_on_chains["BSC"] у CAKE и USDT, формат
    // withdraw_percent_on_chains (с «%» или долей), withdraw_amount_mini, withdraw_eachtime_limit.
}

#[test]
#[ignore = "записать на живом боте: /account/detail, /account/main_keys, /spot/accounts — tests/README.md"]
fn account_and_key() {
    // Записать: key.mode, ip_whitelist, currency_pairs; perms[] (имена и read_only; есть ли имя права вывода);
    // /spot/accounts (currency, available, locked) и ?currency=USDT.
}

#[test]
#[ignore = "записать на живом боте: ордер ioc на минимальную сумму (покупка и продажа CAKE_USDT) — tests/README.md"]
fn ioc_order_fact() {
    // Записать ответ POST /spot/orders (status, finish_as, filled_amount, filled_total, fee, fee_currency, gt_fee,
    // point_fee) и GET /spot/orders/{id}; затем GET /spot/orders?status=finished — поле text.
}

#[test]
#[ignore = "записать на живом боте: адрес депозита, вывод, история выводов и депозитов, spot.balances — tests/README.md"]
fn wallet_private() {
    // Записать: GET /wallet/deposit_address?currency=CAKE (multichain_addresses, min_deposit_amount, min_confirms);
    // POST /withdrawals и GET /wallet/withdrawals?withdraw_id= по шагам статуса; GET /wallet/deposits (txid, статусы
    // TRACK → DONE / DEP_CREDITED); кадры spot.balances при депозите, ордере и выводе (change, freeze_change, change_type).
}
