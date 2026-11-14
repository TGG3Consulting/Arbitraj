//! Файл: tests/bsc_dex_wrapped.rs
//! Что делает: решение 270 — деление WETH() роутеров V2 на нативную и чужую обёртку (pools/wrapped.rs) на записи с цепи
//! (tests/data/fstswap_weth_2026-11-14/calls.json: ответ WETH() роутера fstswap). Проверка: fstswap при заданной
//! dex.native_wrapped = WBNB — чужая обёртка; роутер с WETH() = WBNB — нативная (поведение прежнее); не задана — все
//! нативные. Итог «Проверить подключение» (verdict): три ветки — совпало + чужая, ни один не вернул, роутеров нет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::wrapped, serde_json.

use alloy_primitives::Address;
use arb::venues::bsc_dex::pools::wrapped::{foreign_fact, split, verdict};

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

#[test]
fn verdict_three_branches() {
    let (_, weth) = fixture();
    let wbnb: Address = WBNB.parse().unwrap();
    let fst = ("fstswap".to_string(), weth);
    // Хотя бы один роутер вернул dex.native_wrapped, есть чужая — Ok с фактом.
    let ok = verdict(wbnb, vec![("mdex".to_string(), wbnb), fst.clone()]).unwrap();
    assert!(ok.contains("совпадает с WETH() роутеров V2"));
    assert!(ok.contains(&foreign_fact("fstswap", weth)));
    // Только совпадающие — текст как до решения 270.
    assert_eq!(
        verdict(wbnb, vec![("mdex".to_string(), wbnb)]).unwrap(),
        format!("контракт {wbnb} есть, совпадает с WETH() роутеров V2")
    );
    // Ни один роутер не вернул её — ошибка, как на b517985.
    assert_eq!(
        verdict(wbnb, vec![fst]).unwrap_err(),
        format!("WETH() роутера V2 = {weth}, а dex.native_wrapped = {wbnb} — проверьте адрес")
    );
    // Роутеров нет — сверять не с чем.
    assert!(
        verdict(wbnb, vec![])
            .unwrap()
            .contains("сверить с WETH() не с чем")
    );
}
