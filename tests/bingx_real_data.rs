//! Файл: tests/bingx_real_data.rs
//! Что делает: проверяет разбор сообщений и правил BingX на реально записанных данных
//! (tests/data/bingx, запись 2026-10-23): ping, ответы на подписку, стаканы (сортировка уровней),
//! котировка по уровням, округление ордера по шагам пары.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bingx. Моков нет.

use std::str::FromStr;

use arb::venues::bingx::depth::{self, Msg};
use arb::venues::bingx::{book, symbols, trade};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::Value;

/// Сообщения записи WebSocket.
fn messages() -> Vec<String> {
    let raw = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    raw.lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["m"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[test]
fn ping_and_sub_acks() {
    let m = messages();
    assert!(matches!(depth::parse(&m[0]), Msg::SubAck { code: 0, .. }));
    match depth::parse(&m[2]) {
        Msg::SubAck { id, code, msg } => {
            assert_eq!(id, "t-3");
            assert_eq!(code, 100400);
            assert!(msg.contains("NOSUCH-USDT"));
        }
        _ => panic!("ожидался ответ на подписку с ошибкой"),
    }
    assert!(matches!(depth::parse(&m[5]), Msg::Ping));
}

#[test]
fn depth_levels_sorted_best_first() {
    let m = messages();
    let Msg::Depth {
        symbol,
        bids,
        asks,
        exchange_time,
    } = depth::parse(&m[4])
    else {
        panic!("ожидался стакан")
    };
    assert_eq!(symbol, "CAKE-USDT");
    assert_eq!((asks.len(), bids.len()), (20, 20));
    // BingX прислал asks по убыванию — после разбора лучшая (минимальная) цена первая.
    assert_eq!(asks[0].price, d("2.189"));
    assert_eq!(bids[0].price, d("2.188"));
    assert!(asks.windows(2).all(|w| w[0].price < w[1].price));
    assert!(bids.windows(2).all(|w| w[0].price > w[1].price));
    assert_eq!(exchange_time.unwrap().timestamp_millis(), 1792770730466);
}

#[test]
fn quote_walks_levels() {
    let m = messages();
    let Msg::Depth { asks, .. } = depth::parse(&m[4]) else {
        panic!()
    };
    let (filled, notional) = book::walk(&asks, d("100"));
    assert_eq!(filled, d("100"));
    assert_eq!(
        notional,
        d("11.680") * d("2.189") + d("88.320") * d("2.190")
    );
    // Больше, чем есть в стакане: набирается только доступное.
    let total: Decimal = asks.iter().map(|l| l.qty).sum();
    assert_eq!(book::walk(&asks, total + d("1")).0, total);
}

#[test]
fn symbol_rules_and_rounding() {
    let raw = std::fs::read_to_string("tests/data/bingx/symbols_spot_2026-10-23.json").unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    let map = symbols::parse_list(&v["data"], "USDT").unwrap();
    let cake = &map["CAKE"];
    assert_eq!(cake.symbol, "CAKE-USDT");
    assert_eq!(cake.tick_size, d("0.001"));
    assert_eq!(map["BTC"].step_size, d("0.000001"));
    assert!(cake.tradable(true) && cake.tradable(false));
    assert_eq!(
        trade::round(d("12.3456"), cake.step_size, RoundingStrategy::ToZero),
        d("12.345")
    );
    assert_eq!(
        trade::round(d("2.1897"), cake.tick_size, RoundingStrategy::ToZero),
        d("2.189")
    );
    assert_eq!(
        trade::round(d("2.1891"), cake.tick_size, RoundingStrategy::AwayFromZero),
        d("2.19")
    );
}

/// Решения 202/213 для всех бирж (Тигран 08.11), venues/bingx/book_print.rs — подтверждение тихой пары по содержимому.
/// (1) Разбор ответа GET /openApi/spot/v1/market/depth на ПРИМЕРЕ ИЗ ДОКУМЕНТАЦИИ (docs-v3, Spot → Market Data →
/// Order Book; asks в примере по убыванию — сортируются от лучшей цены): живого ответа REST в tests/data нет.
/// (2) На ЖИВЫХ кадрах записи 2026-10-23: разбор REST (book_print::parse) и разбор кадра (depth.rs) одних и тех же
/// уровней [цена, количество] дают один отпечаток — сравнение ядра (core/feed_confirm.rs verdict) — Quiet; другой кадр
/// (другая книга) — Lost. Пары «кадр + ответ REST в один момент» нет — равенство на одной книге ждёт записи
/// (tests/README.md).
#[test]
fn book_print_docs_example_and_frames() {
    use arb::core::feed_confirm::{Verdict, verdict};
    use arb::gateway::feed_health::{BookPrint, BookPrints, BookVersion, PairHealth};
    use arb::venues::bingx::book_print;
    let docs: Value = serde_json::from_str(
        r#"{"bids":[["42182.22","0.99114"],["42182.19","1.00643"],["42182.17","0.95406"],["42182.10","0.87420"],["42182.08","1.17445"]],"asks":[["42183.23","3.57139"],["42183.22","2.05573"],["42183.20","2.26065"],["42183.19","1.61695"],["42183.17","4.14752"]],"ts":1702720487674}"#,
    )
    .unwrap();
    let p = book_print::parse(&docs, 3).unwrap();
    assert_eq!((p.bids.len(), p.asks.len()), (3, 3));
    assert_eq!(
        (p.asks[0].price, p.asks[0].qty),
        (d("42183.17"), d("4.14752"))
    );
    assert_eq!(
        (p.bids[0].price, p.bids[0].qty),
        (d("42182.22"), d("0.99114"))
    );
    assert_eq!(p.asks[2].price, d("42183.20"));
    // Живые кадры: data кадра — те же поля bids / asks, что у ответа REST.
    let frames: Vec<(Value, BookPrint)> = messages()
        .iter()
        .filter_map(|m| match depth::parse(m) {
            Msg::Depth { bids, asks, .. } => {
                let data = serde_json::from_str::<Value>(m).unwrap()["data"].clone();
                Some((data, BookPrint::top(&bids, &asks, 5)))
            }
            _ => None,
        })
        .collect();
    assert!(frames.len() >= 2, "кадров стакана {}", frames.len());
    let pair = PairHealth {
        ticker: "CAKE".into(),
        last_ms: Some(1),
        since_ms: 0,
        last_id: None,
        last_changed_ms: None,
    };
    let ver = |rest: BookPrint, ours| BookVersion {
        print: Some(BookPrints {
            levels: 5,
            rest,
            ours,
        }),
        ..Default::default()
    };
    for (data, ours) in &frames {
        let rest = book_print::parse(data, 5).unwrap();
        assert_eq!(&rest, ours);
        assert_eq!(
            verdict(&pair, &ver(rest, Some(ours.clone())), 2),
            Verdict::Quiet
        );
    }
    let other = book_print::parse(&frames[0].0, 5).unwrap();
    assert!(matches!(
        verdict(&pair, &ver(other, Some(frames[1].1.clone())), 2),
        Verdict::Lost(_)
    ));
    let bad: Value = serde_json::from_str(r#"{"bids":[],"ts":1}"#).unwrap();
    assert_eq!(book_print::parse(&bad, 5).unwrap_err(), "нет asks");
}
