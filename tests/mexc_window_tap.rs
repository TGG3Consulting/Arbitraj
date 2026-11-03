//! Файл: tests/mexc_window_tap.rs
//! Что делает: разбор кадров окна MEXC (решение 125) на живой записи — когда она появится. Документация MEXC даёт
//! пример сделок только JSON-представлением protobuf (Trade Streams), а не байтами кадра, поэтому тест без записи не
//! пишется (CLAUDE.md: тесты — только на реальных данных); что записать — tests/README.md.
//! Кто вызывает: cargo test -- --ignored.
//! От чего зависит: arb::venues::mexc::tap, arb::gateway::tap.

use arb::core::types::TapView;
use arb::gateway::tap::{TapData, TapStream};
use arb::venues::mexc::tap::decode;

/// Строки файла: hex двоичного кадра (как поле «h» файла окна).
fn frames(path: &str) -> Vec<Vec<u8>> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| hex::decode(l.trim()).unwrap())
        .collect()
}

/// tests/data/mexc_ws/deals.hex — кадры канала spot@public.aggre.deals.v3.api.pb@10ms@<SYMBOL>.
#[test]
#[ignore = "нет живой записи канала сделок MEXC — tests/README.md"]
fn recorded_deals_decode() {
    for b in frames("tests/data/mexc_ws/deals.hex") {
        let r = decode(TapStream::Trades, &TapData::Binary(b)).unwrap();
        let TapView::Trades(t) = r.view else {
            panic!("сделки")
        };
        assert!(
            !t.is_empty()
                && t.iter()
                    .all(|x| !x.id.is_empty() && (x.taker == "buy" || x.taker == "sell"))
        );
        assert!(r.exchange_ms.is_some(), "sendTime обёртки");
    }
}

/// tests/data/mexc_ws/depth.hex — кадры канала spot@public.limit.depth.v3.api.pb@<SYMBOL>@20.
#[test]
#[ignore = "нет живой записи двоичных кадров стакана MEXC — tests/README.md"]
fn recorded_depth_decode() {
    for b in frames("tests/data/mexc_ws/depth.hex") {
        let r = decode(TapStream::Book, &TapData::Binary(b)).unwrap();
        assert!(matches!(r.view, TapView::Book { ref asks, .. } if !asks.is_empty()));
    }
}
