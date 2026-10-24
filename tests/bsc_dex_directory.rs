//! Файл: tests/bsc_dex_directory.rs
//! Что делает: справочник пулов от индексатора (решение 58) на живых ответах GeckoTerminal 2026-10-24
//! (tests/data/indexers_2026-10-24): разбор пулов монеты и списка DEX, PoolId у V4/Infinity, страница за пределами
//! (HTTP 400 на странице 3) — «страниц больше нет», коды ошибок, отбор пар с USDT и промежуточной монетой.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::directory.
//! Настройки: нет.

use std::collections::HashSet;
use std::str::FromStr;

use alloy_primitives::Address;
use arb::core::types::DirectoryStatus;
use arb::gateway::errors::ErrorCode;
use arb::venues::bsc_dex::directory::gt_http::{Got, answer};
use arb::venues::bsc_dex::directory::{Directory, TokenEntry, gt_parse, is_relevant};
use serde_json::Value;

const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";
const WBNB: &str = "0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c";
const CAKE: &str = "0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82";

/// Запись из файла: (HTTP-код, тело).
fn rec(name: &str) -> (u16, String) {
    let text =
        std::fs::read_to_string("tests/data/indexers_2026-10-24/arb_indexers_2026-10-24.json")
            .expect("файл записи");
    let all: Value = serde_json::from_str(&text).expect("JSON");
    let r = &all[name];
    (
        r["status"].as_u64().expect("status") as u16,
        r["body"].as_str().expect("body").to_string(),
    )
}

fn addr(s: &str) -> Address {
    Address::from_str(s).unwrap()
}

#[test]
fn dexes_two_pages_and_page_end() {
    let (s1, b1) = rec("gt_dexes_bsc_p1");
    let (s2, b2) = rec("gt_dexes_bsc_p2");
    assert_eq!((s1, s2), (200, 200));
    let (l1, next1) = gt_parse::dexes(&b1).unwrap();
    let (l2, next2) = gt_parse::dexes(&b2).unwrap();
    assert!(
        next1 && !next2,
        "links.next: есть на первой, null на второй"
    );
    assert_eq!(l1.len() + l2.len(), 139);
    let ids: HashSet<String> = l1.iter().chain(&l2).map(|d| d.id.clone()).collect();
    for id in [
        "pancakeswap_v2",
        "pancakeswap-v3-bsc",
        "uniswap-bsc",
        "uniswap-v2-bsc",
        "uniswap-v4-bsc",
        "pancakeswap-infinity-clmm",
        "pancakeswap-infinity-lbamm-bsc",
    ] {
        assert!(ids.contains(id), "{id}");
    }
    let uni = l1
        .iter()
        .chain(&l2)
        .find(|d| d.id == "uniswap-bsc")
        .unwrap();
    assert_eq!(uni.name, "Uniswap V3 (BSC)");

    // Страница 3 — HTTP 400 «expected :page in 1..2; got 3»: на странице > 1 это конец списка, а не ошибка.
    let (s3, b3) = rec("gt_dexes_bsc_p3");
    assert_eq!(s3, 400);
    match answer(s3, 3, &b3, "/networks/bsc/dexes?page=3").unwrap() {
        Got::PageEnd(t) => assert_eq!(t, "expected :page in 1..2; got 3"),
        Got::Body(_) => panic!("400 — не тело"),
    }
    // На первой странице 400 — ошибка с текстом индексатора.
    let e = answer(400, 1, &b3, "/x").unwrap_err();
    assert_eq!(e.code, ErrorCode::BadResponse);
    assert!(
        e.message.contains("expected :page in 1..2"),
        "{}",
        e.message
    );
}

#[test]
fn http_codes() {
    assert_eq!(
        answer(429, 1, "", "/x").unwrap_err().code,
        ErrorCode::RateLimited
    );
    assert_eq!(
        answer(401, 1, "", "/x").unwrap_err().code,
        ErrorCode::InvalidCredentials
    );
    assert_eq!(
        answer(503, 2, "", "/x").unwrap_err().code,
        ErrorCode::Unreachable
    );
    assert!(matches!(answer(200, 1, "{}", "/x").unwrap(), Got::Body(_)));
}

#[test]
fn pools_of_five_coins() {
    for c in ["CAKE", "ETH", "XRP", "TWT", "DOGE"] {
        let (s, b) = rec(&format!("gt_pools_{c}_p1"));
        assert_eq!(s, 200);
        let list = gt_parse::pools(&b).unwrap();
        assert_eq!(list.len(), 20, "{c}: 20 пулов на странице");
        for p in &list {
            // V2/V3 — адрес контракта (42 знака), V4/Infinity — PoolId (66 знаков).
            let v4 = p.dex_id == "uniswap-v4-bsc" || p.dex_id.starts_with("pancakeswap-infinity");
            assert_eq!(
                p.pool.len(),
                if v4 { 66 } else { 42 },
                "{c} {} {}",
                p.dex_id,
                p.pool
            );
        }
    }
    let (_, b) = rec("gt_pools_CAKE_p1");
    let list = gt_parse::pools(&b).unwrap();
    let p = list
        .iter()
        .find(|p| p.pool == "0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1")
        .unwrap();
    assert_eq!(
        (p.dex_id.as_str(), p.base, p.quote),
        ("pancakeswap-v3-bsc", addr(CAKE), addr(USDT))
    );
    assert_eq!(
        (p.base_symbol.as_str(), p.quote_symbol.as_str()),
        ("Cake", "USDT")
    );
    assert_eq!(p.reserve_usd.unwrap().to_string(), "6210893.2214");
    // Пул Infinity с нативной BNB: валюта — нулевой адрес (это не WBNB).
    let native = list
        .iter()
        .find(|p| p.pool.starts_with("0x54c27041"))
        .unwrap();
    assert_eq!(native.quote, Address::ZERO);
}

#[test]
fn relevant_pairs_only() {
    let (_, b) = rec("gt_pools_CAKE_p1");
    let list = gt_parse::pools(&b).unwrap();
    let cake = addr(CAKE);
    let only_usdt: HashSet<Address> = [addr(USDT)].into();
    let with_wbnb: HashSet<Address> = [addr(USDT), addr(WBNB)].into();
    let n = |q: &HashSet<Address>| list.iter().filter(|p| is_relevant(cake, p, q)).count();
    // CAKE/USDT: PancakeSwap V3 ×2, Uniswap V3, Infinity CL ×2, PancakeSwap V2.
    assert_eq!(n(&only_usdt), 6);
    // + CAKE/WBNB: PancakeSwap V2, V3 ×2, V1 (Infinity с нативной BNB — не WBNB).
    assert_eq!(n(&with_wbnb), 10);

    let d = Directory::default();
    d.with(|s| {
        s.quotes = only_usdt.clone();
        s.tokens.insert(
            cake,
            TokenEntry {
                ticker: "CAKE".into(),
                pools: list.clone(),
                pages: 1,
                ..Default::default()
            },
        );
    });
    let v = d.view(None, None, (None, None));
    assert_eq!(v.tokens[0].total, 20);
    assert_eq!(v.pools.len(), 6);
    assert!(
        v.pools
            .iter()
            .all(|p| p.status == DirectoryStatus::Unchecked)
    );
    assert_eq!(d.relevant().len(), 6);
}
