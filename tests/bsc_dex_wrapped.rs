//! Файл: tests/bsc_dex_wrapped.rs
//! Что делает: решение 270 — деление WETH() роутеров V2 на нативную и чужую обёртку (pools/wrapped.rs) на записи с цепи
//! (tests/data/fstswap_weth_2026-11-14/calls.json: ответ WETH() роутера fstswap). Проверка: fstswap при заданной
//! dex.native_wrapped = WBNB — чужая обёртка; роутер с WETH() = WBNB — нативная (поведение прежнее); не задана — все
//! нативные.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::wrapped, serde_json.

use alloy_primitives::Address;
use arb::venues::bsc_dex::pools::wrapped::{foreign_fact, split};

const WBNB: &str = "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c";

fn fixture() -> (Address, Address) {
    let v: serde_json::Value =
        serde_json::from_str(include_str!("data/fstswap_weth_2026-11-14/calls.json")).unwrap();
    let router = v["router"].as_str().unwrap().parse().unwrap();
    let raw = v["WETH()_0xad5c4648"].as_str().unwrap();
    (router, format!("0x{}", &raw[26..]).parse().unwrap())
}

#[test]
fn fstswap_weth_is_not_wbnb() {
    let (_, weth) = fixture();
    let wbnb: Address = WBNB.parse().unwrap();
    assert_ne!(weth, wbnb);
    let list = vec![("fstswap".to_string(), weth), ("mdex".to_string(), wbnb)];
    let (own, foreign) = split(list, Some(wbnb));
    assert_eq!(own, vec![wbnb]);
    assert_eq!(foreign, vec![("fstswap".to_string(), weth)]);
    assert!(foreign_fact("fstswap", weth).contains("не dex.native_wrapped"));
}

#[test]
fn wbnb_routers_unchanged() {
    let wbnb: Address = WBNB.parse().unwrap();
    let list = vec![("a".to_string(), wbnb), ("b".to_string(), wbnb)];
    let (own, foreign) = split(list, Some(wbnb));
    assert_eq!(own, vec![wbnb, wbnb]);
    assert!(foreign.is_empty());
}

#[test]
fn native_not_set_keeps_old_behaviour() {
    let (_, weth) = fixture();
    let (own, foreign) = split(vec![("fstswap".to_string(), weth)], None);
    assert_eq!(own, vec![weth]);
    assert!(foreign.is_empty());
}
