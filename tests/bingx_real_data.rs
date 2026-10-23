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
    assert!(matches!(
        depth::parse(&m[0]),
        Ok(Msg::SubAck { code: 0, .. })
    ));
    match depth::parse(&m[2]) {
        Ok(Msg::SubAck { code, msg }) => {
            assert_eq!(code, 100400);
            assert!(msg.contains("NOSUCH-USDT"));
        }
        _ => panic!("ожидался ответ на подписку с ошибкой"),
    }
    assert!(matches!(depth::parse(&m[5]), Ok(Msg::Ping)));
}

#[test]
fn depth_levels_sorted_best_first() {
    let m = messages();
    let Ok(Msg::Depth {
        symbol,
        bids,
        asks,
        exchange_time,
    }) = depth::parse(&m[4])
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
    let Ok(Msg::Depth { asks, .. }) = depth::parse(&m[4]) else {
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
