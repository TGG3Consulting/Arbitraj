//! Файл: tests/bybit_real_data.rs
//! Что делает: адаптер Bybit (решение 136) на ЖИВЫХ публичных ответах Bybit от 2026-11-04 (сервер, curl/websockets как
//! есть; tests/data/cex_samples_2026_11_04/bybit_*): справочник спот-пар instruments-info, правила ордера, поток
//! wss://stream.bybit.com/v5/public/spot 20 с (orderbook.50.CAKEUSDT — snapshot + delta с u и seq, publicTrade.CAKEUSDT,
//! ответы subscribe и pong). Приватные ответы (ключ) — тесты #[ignore] внизу: что записать на живом боте.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bybit. Моков нет.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{Side, TapView};
use arb::gateway::tap::{TapData, TapStream};
use arb::venues::bybit::depth::{self, Msg};
use arb::venues::bybit::local_book::{Applied, LocalBook};
use arb::venues::bybit::{order_rules, symbols, tap};

const DIR: &str = "tests/data/cex_samples_2026_11_04";

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).expect("число")
}

/// result ответа instruments-info.
fn instruments() -> Value {
    let t = std::fs::read_to_string(format!("{DIR}/bybit_instruments_spot.json")).expect("запись");
    let v: Value = serde_json::from_str(&t).expect("JSON");
    assert_eq!(v["retCode"], 0);
    v["result"].clone()
}

/// Кадры потока (поле msg), по порядку записи.
fn ws_frames() -> Vec<String> {
    let t = std::fs::read_to_string(format!("{DIR}/bybit_ws.jsonl")).expect("запись");
    t.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v.get("msg").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// Живой справочник: 520 пар, все Trading; CAKEUSDT — шаг 0.001 (basePrecision), цена 0.0001 (tickSize), минимум
/// суммы 5 USDT (minOrderAmt), наибольшее количество 8457 (maxLimitOrderQty).
#[test]
fn instruments_live_parse() {
    let map = symbols::parse_list(&instruments(), "USDT").expect("разбор");
    let cake = &map["CAKE"];
    assert_eq!(cake.symbol, "CAKEUSDT");
    assert!(cake.tradable());
    assert_eq!(cake.step_size, dec("0.001"));
    assert_eq!(cake.tick_size, dec("0.0001"));
    assert_eq!(cake.min_notional, Some(dec("5")));
    assert_eq!(cake.max_qty, Some(dec("8457")));
    assert_eq!(map["BTC"].tick_size, dec("0.1"));
    assert!(map.values().all(|r| r.tradable()));
    assert!(map.len() > 300, "пар с USDT: {}", map.len());
}

/// Правила ордера на живых правилах CAKEUSDT: количество вниз до шага, цена покупки вниз и продажи вверх до тика,
/// сумма меньше 5 USDT и количество больше maxLimitOrderQty — отказ.
#[test]
fn order_rules_on_live_cake() {
    let map = symbols::parse_list(&instruments(), "USDT").expect("разбор");
    let r = &map["CAKE"];
    let (q, p) =
        order_rules::check_rules(r, Side::Buy, dec("10.12345"), dec("1.89456")).expect("ок");
    assert_eq!((q, p), (dec("10.123"), dec("1.8945")));
    let (_, p) = order_rules::check_rules(r, Side::Sell, dec("10"), dec("1.89451")).expect("ок");
    assert_eq!(p, dec("1.8946"));
    assert!(order_rules::check_rules(r, Side::Buy, dec("2"), dec("1.89")).is_err());
    assert!(order_rules::check_rules(r, Side::Buy, dec("9000"), dec("1.89")).is_err());
    let id = order_rules::link_id(true, "20261104123456789CAKE-long-deal-id");
    assert!(id.len() <= order_rules::MAX_LINK_ID);
}

/// Живой поток: ответ subscribe и pong спота разбираются как op; стакан — snapshot, затем только delta, u растёт на 1
/// (запись 20 с — наблюдение, в документации не обещано); локальный стакан без разрывов, bids по убыванию, asks по
/// возрастанию, лучший bid < лучшего ask, не больше 50 уровней.
#[test]
fn orderbook_stream_live() {
    let mut book = LocalBook::default();
    let (mut snaps, mut deltas, mut acks, mut last_u) = (0, 0, 0, None::<i64>);
    for f in ws_frames() {
        match depth::parse(&f) {
            Msg::Ack { op, success, .. } => {
                assert!(success, "{f}");
                assert!(op == "subscribe" || op == "ping");
                acks += 1;
            }
            Msg::Book {
                symbol,
                snapshot,
                u,
                ts,
                bids,
                asks,
            } => {
                assert_eq!(symbol, "CAKEUSDT");
                assert!(ts.is_some());
                if snapshot {
                    snaps += 1;
                } else {
                    deltas += 1;
                    assert_eq!(Some(u - 1), last_u, "u подряд");
                }
                assert_eq!(
                    book.apply(snapshot, u, true, &bids, &asks),
                    Applied::Changed
                );
                last_u = Some(u);
                let (b, a) = book.top(50);
                assert!(b.windows(2).all(|w| w[0].price > w[1].price));
                assert!(a.windows(2).all(|w| w[0].price < w[1].price));
                assert!(b[0].price < a[0].price);
                assert!(b.len() <= 50 && a.len() <= 50);
            }
            Msg::Bad(r) => panic!("кадр не разобран: {r}: {f}"),
            Msg::Other => {}
        }
    }
    assert_eq!(snaps, 1);
    assert!(deltas > 10, "изменений {deltas}");
    assert!(acks >= 2);
    // Повтор того же u у снимка — Repeat; изменение с тем же u — разрыв.
    let u = last_u.expect("u");
    assert_eq!(book.apply(true, u, false, &[], &[]), Applied::Repeat);
    assert!(matches!(
        book.apply(false, u, false, &[], &[]),
        Applied::Gap(_)
    ));
    assert!(
        matches!(book.apply(false, u + 1, false, &[], &[]), Applied::Gap(_)),
        "после разрыва — только снимок"
    );
}

/// Строгая проверка u (venue.book_seq_strict, решение 136, Тигран 04.11) на живом потоке с одним выброшенным кадром
/// delta (так выглядит потерянный кадр): строго — разрыв на следующем кадре; без строгой — только рост u, стакан идёт.
#[test]
fn orderbook_strict_seq_live_skip() {
    let frames = ws_frames();
    let is_delta = |f: &String| {
        matches!(
            depth::parse(f),
            Msg::Book {
                snapshot: false,
                ..
            }
        )
    };
    let skip = frames.iter().position(is_delta).expect("есть delta");
    let run = |strict: bool| {
        let mut book = LocalBook::default();
        let mut gaps = 0;
        for (i, f) in frames.iter().enumerate() {
            if i == skip {
                continue;
            }
            if let Msg::Book {
                snapshot,
                u,
                bids,
                asks,
                ..
            } = depth::parse(f)
                && matches!(
                    book.apply(snapshot, u, strict, &bids, &asks),
                    Applied::Gap(_)
                )
            {
                gaps += 1;
            }
        }
        gaps
    };
    assert!(run(true) >= 1, "строгая проверка видит пропуск");
    assert_eq!(run(false), 0, "без строгой — только рост u");
}

/// Разбор кадров окна на живой записи: сделки publicTrade (S Buy/Sell → buy/sell, i, T), стакан Book — снимок как Book,
/// изменение — сводкой; Diff — снимок full и изменение с to = u.
#[test]
fn window_frames_live() {
    let (mut trades, mut diffs) = (0, 0);
    for f in ws_frames() {
        let data = TapData::Text(f.clone());
        if f.contains("\"publicTrade.") {
            let d = tap::decode(TapStream::Trades, &data).expect("сделки");
            let TapView::Trades(list) = d.view else {
                panic!("не сделки")
            };
            assert!(!list.is_empty());
            assert!(list.iter().all(|t| t.taker == "buy" || t.taker == "sell"));
            assert!(
                list.iter()
                    .all(|t| !t.id.is_empty() && t.time_ms > 0 && t.qty > Decimal::ZERO)
            );
            trades += 1;
        } else if f.contains("\"orderbook.") {
            let b = tap::decode(TapStream::Book, &data).expect("стакан");
            let full = f.contains("\"snapshot\"");
            assert_eq!(matches!(b.view, TapView::Book { .. }), full);
            let d = tap::decode(TapStream::Diff, &data).expect("изменения");
            let TapView::Diff { full: f2, to, .. } = d.view else {
                panic!("не Diff")
            };
            assert_eq!(f2, full);
            assert!(to.is_some());
            diffs += 1;
        }
    }
    assert!(trades > 0 && diffs > 0);
}

/// Деление монет на соединения: не больше per_conn на соединение.
#[test]
fn market_split_limits() {
    use arb::core::types::Asset;
    let list: Vec<(Asset, String)> = (0..250)
        .map(|i| {
            (
                Asset {
                    ticker: format!("C{i}"),
                    bsc_contract: None,
                },
                format!("C{i}USDT"),
            )
        })
        .collect();
    let chunks = arb::venues::bybit::market::split(&list, 100, 50);
    assert_eq!(
        chunks.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![100, 100, 50]
    );
}

// ---------- Приватные ответы: записать на живом боте (ключ Bybit; tests/README.md) ----------

/// Записать: GET /v5/asset/coin/query-info (весь ответ, ~МБ) и ?coin=CAKE, ?coin=USDT в
/// tests/data/bybit_private/coin_info*.json. Проверить: имя сети BSC в chain (для venue.bsc_network), contractAddress у
/// BSC совпадает с адресом токена в пуле, chainDeposit/chainWithdraw — строки "0"/"1", withdrawPercentageFee у BSC,
/// minAccuracy, withdrawMax "-1".
#[test]
#[ignore = "нет записи coin/query-info (приватный) — записать на живом боте"]
fn coin_info_live() {
    let t = std::fs::read_to_string("tests/data/bybit_private/coin_info.json").expect("запись");
    let v: Value = serde_json::from_str(&t).expect("JSON");
    let venue = arb::core::types::VenueId("bybit".into());
    let p = arb::venues::bybit::networks_parse::parse_networks(&v["result"], "BSC", &venue)
        .expect("разбор");
    assert!(p.skipped.is_empty(), "{:?}", p.skipped);
}

/// Записать: GET /v5/account/wallet-balance?accountType=UNIFIED, GET /v5/user/query-api, GET /v5/account/info —
/// права ключа (Spot: SpotTrade; Wallet: AccountTransfer, Withdraw; пустые ContractTrade/Options/Derivatives), uta,
/// isMaster, unifiedMarginStatus, spotBorrow и locked у монет.
#[test]
#[ignore = "нет записи wallet-balance / query-api / account/info — записать на живом боте"]
fn account_live() {}

/// Записать: первый IOC-ордер Limit спота (POST /v5/order/create — принят ли timeInForce IOC у спота), кадры
/// order.spot и execution.spot приватного потока, GET /v5/order/realtime?orderId=, GET /v5/execution/list?orderId= —
/// orderStatus, cumExecQty, cumExecValue, cumFeeDetail (монета комиссии), execFee/feeCurrency, сумма execQty.
#[test]
#[ignore = "нет записи ордера и приватного потока — записать на живом боте"]
fn order_and_private_stream_live() {}

/// Записать: первый депозит по BSC (GET /v5/asset/deposit/query-record?txID= — status 3 или 10012, depositType),
/// перевод FUND → UNIFIED (POST /v5/asset/transfer/inter-transfer — status), вывод (POST /v5/asset/withdraw/create и
/// GET /v5/asset/withdraw/query-record?withdrawID= — статусы и txID), адрес депозита (query-address — tagDeposit).
#[test]
#[ignore = "нет записей депозита, перевода и вывода — записать на живом боте"]
fn transfers_live() {}

/// Решения 202/213 для всех бирж (Тигран 08.11), venues/bybit/book_print.rs: разбор ЖИВОГО ответа GET /v5/market/orderbook
/// (bybit_orderbook_CAKEUSDT.json, limit 50) — первые N уровней от лучшей цены, u ответа; живая логика ядра
/// (core/feed_confirm.rs verdict): ответ против самого себя — Quiet; против локального стакана из записанного потока
/// orderbook.50 (снимок + изменения через 45–55 с после ответа REST — книга за это время менялась) — Lost.
/// Почему у глубины 50 не номер: u ответа REST и u кадров orderbook.50 на записи — разные счётчики (документация
/// связывает u REST только с потоком 1000 уровней). Пары «кадр + ответ REST в один момент» в записи нет — равенство
/// отпечатков на одной книге ждёт записи (tests/README.md).
#[test]
fn book_print_live_orderbook() {
    use arb::core::feed_confirm::{Verdict, verdict};
    use arb::gateway::feed_health::{BookPrint, BookPrints, BookVersion, PairHealth};
    use arb::venues::bybit::book_print;
    let t =
        std::fs::read_to_string(format!("{DIR}/bybit_orderbook_CAKEUSDT.json")).expect("запись");
    let v: Value = serde_json::from_str(&t).expect("JSON");
    let r = &v["result"];
    let rest = book_print::parse(r, 50).expect("разбор");
    assert_eq!((rest.bids.len(), rest.asks.len()), (50, 50));
    assert_eq!(
        (rest.asks[0].price, rest.asks[0].qty),
        (dec("1.8938"), dec("6.004"))
    );
    assert_eq!(
        (rest.bids[0].price, rest.bids[0].qty),
        (dec("1.8934"), dec("6.194"))
    );
    assert!(rest.bids.windows(2).all(|w| w[0].price > w[1].price));
    assert!(rest.asks.windows(2).all(|w| w[0].price < w[1].price));
    assert_eq!(book_print::parse(r, 5).expect("разбор").asks.len(), 5);
    let rest_u = book_print::u_of(r).expect("u");
    assert_eq!(rest_u, 3_200_727);
    // Локальный стакан из записанного потока orderbook.50 (как ws_conn.rs).
    let mut book = LocalBook::default();
    let mut stream_u = vec![];
    for f in ws_frames() {
        if let Msg::Book {
            snapshot,
            u,
            bids,
            asks,
            ..
        } = depth::parse(&f)
        {
            book.apply(snapshot, u, true, &bids, &asks);
            stream_u.push(u);
        }
    }
    assert!(
        !stream_u.is_empty() && stream_u.iter().all(|&u| u < rest_u),
        "u потока 50 и u REST — разные счётчики"
    );
    let (b, a) = book.top(50);
    let ours = BookPrint::top(&b, &a, 50);
    let pair = PairHealth {
        ticker: "CAKE".into(),
        last_ms: Some(1),
        since_ms: 0,
        last_id: None,
        last_changed_ms: None,
    };
    let ver = |ours| BookVersion {
        print: Some(BookPrints {
            levels: 50,
            rest: rest.clone(),
            ours,
        }),
        ..Default::default()
    };
    assert_eq!(verdict(&pair, &ver(Some(rest.clone())), 2), Verdict::Quiet);
    assert!(matches!(
        verdict(&pair, &ver(Some(ours)), 2),
        Verdict::Lost(_)
    ));
    assert!(matches!(verdict(&pair, &ver(None), 2), Verdict::Lost(_)));
    // Нет стороны — ошибка с причиной.
    let bad: Value = serde_json::from_str(r#"{"s":"CAKEUSDT","b":[],"u":1}"#).unwrap();
    assert_eq!(book_print::parse(&bad, 5).unwrap_err(), "нет a");
}
