//! Файл: tests/bingx_window_tap.rs
//! Что делает: разбор кадров окна BingX (решение 125): (1) записанные кадры стакана @depth5/@depth20
//! (tests/data/bingx/ws_spot_2026-10-23.jsonl) — уровни отсортированы, время BingX — поле timestamp; (2) сделка канала
//! @trade — на примере сообщения из официальной документации docs-v3 (Spot → Websocket Market Data → «Subscription
//! transaction by transaction», dataUpdateDemo; живой записи канала @trade нет); (3) строка кадра в файле окна
//! (core/window_tap.rs) переводится обратно в тот же кадр — текст и двоичное (hex); (4) живая запись ленты сделок —
//! когда появится (#[ignore], что записать — tests/README.md).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bingx::tap, arb::core::window_tap, arb::gateway::tap.

use arb::core::types::{TapView, VenueId};
use arb::core::window_tap::{line, parse};
use arb::gateway::tap::{TapData, TapFrame, TapStream};
use arb::venues::bingx::tap::decode;
use rust_decimal::Decimal;
use serde_json::Value;
use std::str::FromStr;

/// Тексты сообщений записи BingX.
fn recorded() -> Vec<String> {
    std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl")
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["m"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn recorded_book_frames_decode() {
    let books: Vec<String> = recorded()
        .into_iter()
        .filter(|m| m.contains("@depth") && m.contains("\"data\""))
        .collect();
    assert!(!books.is_empty());
    for m in &books {
        let v: Value = serde_json::from_str(m).unwrap();
        let r = decode(TapStream::Book, &TapData::Text(m.clone())).unwrap();
        assert_eq!(r.exchange_ms, v["timestamp"].as_i64());
        let TapView::Book { bids, asks } = r.view else {
            panic!("стакан: {m}")
        };
        assert_eq!(asks.len(), v["data"]["asks"].as_array().unwrap().len());
        assert!(
            asks.windows(2).all(|w| w[0].price <= w[1].price),
            "asks по возрастанию"
        );
        assert!(
            bids.windows(2).all(|w| w[0].price >= w[1].price),
            "bids по убыванию"
        );
    }
    // Ответ на подписку — не стакан.
    let ack = recorded()
        .into_iter()
        .find(|m| m.contains("SUCCESS"))
        .unwrap();
    assert!(matches!(
        decode(TapStream::Book, &TapData::Text(ack)).unwrap().view,
        TapView::Other(_)
    ));
}

/// Пример push канала @trade из docs-v3 (dataUpdateDemo; в исходнике страницы — объект JS, m: !0 = true).
const DOCS_TRADE: &str = r#"{"code":0,"data":{"E":1760066129336,"T":1760066129316,"e":"trade","m":true,"p":"121123.47","q":"0.003189","s":"BTC-USDT","t":"172356086"},"dataType":"BTC-USDT@trade","success":true,"timestamp":1760066129336}"#;

#[test]
fn docs_trade_example_decodes() {
    let r = decode(TapStream::Trades, &TapData::Text(DOCS_TRADE.into())).unwrap();
    assert_eq!(r.exchange_ms, Some(1760066129336));
    let TapView::Trades(t) = r.view else {
        panic!("сделки")
    };
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].id, "172356086");
    assert_eq!(t[0].price, Decimal::from_str("121123.47").unwrap());
    assert_eq!(t[0].qty, Decimal::from_str("0.003189").unwrap());
    assert_eq!(t[0].time_ms, 1760066129316);
    assert_eq!(
        t[0].taker, "sell",
        "m = true: buyer is maker — активная продажа"
    );
}

#[test]
fn frame_line_round_trip() {
    let text = recorded()
        .into_iter()
        .find(|m| m.contains("@depth"))
        .unwrap();
    for data in [
        TapData::Text(text.clone()),
        TapData::Binary(text.into_bytes()),
    ] {
        let f = TapFrame {
            venue: VenueId("bingx".into()),
            stream: TapStream::Book,
            conn: "c1".into(),
            recv_ns: 1_792_770_724_358_123_456,
            data,
        };
        let back = parse(&line(&f)).unwrap();
        assert_eq!(
            (back.venue, back.stream, back.conn, back.recv_ns),
            (f.venue, f.stream, f.conn, f.recv_ns)
        );
        assert_eq!(back.data, f.data);
    }
}

/// Живая запись ленты сделок BingX: tests/data/bingx_trades/trades.jsonl — строки {"m": текст после GZIP} канала
/// <SYMBOL>@trade (как пишет feed.rs). Записать на сервере бота: окно с монетой BingX или сырой журнал соединения.
#[test]
#[ignore = "нет живой записи канала @trade — tests/README.md"]
fn recorded_trades_decode() {
    let text = std::fs::read_to_string("tests/data/bingx_trades/trades.jsonl").unwrap();
    for l in text.lines() {
        let m = serde_json::from_str::<Value>(l).unwrap()["m"]
            .as_str()
            .unwrap()
            .to_string();
        let r = decode(TapStream::Trades, &TapData::Text(m.clone())).unwrap();
        assert!(
            matches!(r.view, TapView::Trades(ref t) if !t.is_empty()),
            "{m}"
        );
    }
}
