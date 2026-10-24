//! Файл: tests/bsc_dex_subs_live.rs
//! Что делает: подписки пулов DEX на живых данных 2026-10-24 (tests/data/live_2026-10-24/bsc_dex_subs.json — выборка из
//! сырого журнала узла BSC и журнала SQLite того же запуска):
//! 1) пулы истории V4 — пачками на одном соединении (attach::split): 1968 подключений из журнала (каждое раньше было
//!    своим соединением с подпиской) раскладываются в ceil(пулов / dex.pools_per_sub) пачек на вид;
//! 2) мелкий пул слушает только добавление ликвидности (group::filter, Topics::Liquidity): topic0 Mint — тот же, что
//!    в реальном логе узла, а Swap (346 из 356 уведомлений пула AIN/USDT за окно) в фильтр не входит; полный фильтр — все;
//! 3) topic0 событий V4 (Uniswap V4 ModifyLiquidity, Infinity Bin) из реальных логов — в нужных списках.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools (attach, group, events, families, discover). Моков нет.

use std::collections::BTreeMap;
use std::str::FromStr;

use alloy_primitives::{Address, B256};
use serde_json::Value;

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::pools::attach::split;
use arb::venues::bsc_dex::pools::discover::Found;
use arb::venues::bsc_dex::pools::events;
use arb::venues::bsc_dex::pools::families::FAMILIES;
use arb::venues::bsc_dex::pools::group::{Topics, filter};

fn fixture() -> Value {
    let raw = std::fs::read_to_string("tests/data/live_2026-10-24/bsc_dex_subs.json").unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn addr(v: &Value) -> Address {
    Address::from_str(v.as_str().unwrap()).unwrap()
}

fn b256(v: &Value) -> B256 {
    B256::from_str(v.as_str().unwrap()).unwrap()
}

/// topic0 из фильтра logs (поле topics[0]).
fn topic0s(filter: &Value) -> Vec<B256> {
    filter["topics"][0]
        .as_array()
        .unwrap()
        .iter()
        .map(b256)
        .collect()
}

#[test]
fn v4_history_pools_go_to_few_groups() {
    let fx = fixture();
    let per = fx["settings_bsc_dex"]["dex.pools_per_sub"]
        .as_u64()
        .unwrap() as usize;
    let att = fx["journal_v4_attach"].as_array().unwrap();
    assert_eq!(
        att.len(),
        1968,
        "подключений в журнале — каждое было своим соединением"
    );
    // Повтор раскладки attach.rs по порядку журнала: открытая пачка вида дополняется, пока есть место.
    let mut open: BTreeMap<String, usize> = BTreeMap::new();
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    let mut total: BTreeMap<String, usize> = BTreeMap::new();
    for a in att {
        for (family, n) in a[1].as_object().unwrap() {
            let n = n.as_u64().unwrap() as usize;
            *total.entry(family.clone()).or_default() += n;
            let (into, new) = split(open.get(family).copied(), n, per);
            if let Some(c) = open.get_mut(family) {
                *c += into;
            }
            for size in new {
                open.insert(family.clone(), size);
                *groups.entry(family.clone()).or_default() += 1;
            }
        }
    }
    for (family, n) in &total {
        assert_eq!(groups[family], n.div_ceil(per), "{family}: {n} пулов");
    }
    let all: usize = groups.values().sum();
    assert!(all <= 13, "соединений {all} вместо 1968");
}

#[test]
fn split_respects_open_group_and_limit() {
    assert_eq!(split(None, 450, 200), (0, vec![200, 200, 50]));
    assert_eq!(split(Some(150), 120, 200), (50, vec![70]));
    assert_eq!(split(Some(200), 3, 200), (0, vec![3]));
    assert_eq!(split(Some(10), 0, 200), (0, vec![]));
}

#[test]
fn shallow_pool_hears_only_liquidity_adds() {
    let fx = fixture();
    let s = &fx["shallow_pool"];
    let f = FAMILIES.iter().find(|f| f.id == s["family"]).unwrap();
    let pool = addr(&s["pool"]);
    let (coin, usdt) = (addr(&s["coin"]), addr(&fx["usdt"]));
    let (token0, token1) = if coin < usdt {
        (coin, usdt)
    } else {
        (usdt, coin)
    };
    let found = Found {
        family: f,
        reference: PoolRef::Contract(pool),
        token0,
        token1,
        coin,
        fee_pips: s["fee"].as_u64().map(|x| x as u32),
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    };
    let swap = b256(&s["swap_log"]["topics"][0]);
    let mint = b256(&s["mint_log"]["topics"][0]);
    assert_eq!(addr(&s["swap_log"]["address"]), pool);
    let thin = filter(&[&found], Topics::Liquidity);
    assert_eq!(
        thin["address"][0].as_str().unwrap().to_lowercase(),
        s["pool"]
    );
    assert_eq!(topic0s(&thin), vec![mint], "только Mint из реального лога");
    let full = filter(&[&found], Topics::Full);
    assert!(topic0s(&full).contains(&swap) && topic0s(&full).contains(&mint));
    // За окно журнала: 346 Swap не придут, 5 Mint — придут (будят пул), Burn — не нужен.
    let c = &s["counts_in_window"];
    assert_eq!(c["swap"].as_u64(), Some(346));
    assert!(c["mint"].as_u64().unwrap() < c["swap"].as_u64().unwrap());
}

#[test]
fn v4_live_topics_are_in_their_lists() {
    let fx = fixture();
    let u4 = FAMILIES.iter().find(|f| f.id == "uniswap_v4").unwrap();
    let ml = b256(&fx["uniswap_v4_modify_liquidity_log"]["topics"][0]);
    assert!(events::topics(u4).contains(&ml));
    assert_eq!(events::liquidity_topics(u4), vec![ml]);
    let bin = FAMILIES
        .iter()
        .find(|f| f.id == "pancakeswap_infinity_bin")
        .unwrap();
    let t = b256(&fx["infinity_bin_log"]["topics"][0]);
    assert!(
        events::topics(bin).contains(&t),
        "событие Bin из журнала слушается"
    );
}
