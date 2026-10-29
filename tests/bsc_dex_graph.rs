//! Файл: tests/bsc_dex_graph.rs
//! Что делает: проверяет граф токенов в ОЗУ (quote/pools graph.rs, решение 80): построение снимком из реестра,
//! индекс смежности «токен → рёбра», отсев мелких пулов (глубина ниже dex.min_pool_liquidity_usd, решение 73) и
//! ограничение dex.route_max_pools_per_edge (на пару остаются самые глубокие). Рёбра — пары V2; глубина пула по
//! направлениям уже проверена в tests/bsc_dex_depth_*.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools, parts}, arb::settings (SQLite во временной папке). Резервы пар
//! заданы числами — узлы BSC из песочницы недоступны; проверяется построение графа, а не рынок.

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::types::{Asset, Pool, PoolRef, PoolState, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::{families, graph};

const USDT: Address = address!("0x0000000000000000000000000000000000000055");
const COIN: Address = address!("0x00000000000000000000000000000000000000c0");
const WBNB: Address = address!("0x00000000000000000000000000000000000000b0");

/// Окружение: настройки глубины (решение 73) и предел пулов на ребро.
async fn ctx(max_per_edge: i64) -> Ctx {
    let d = std::env::temp_dir().join(format!(
        "arb-dex-graph-{}-{max_per_edge}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let c = Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    };
    let v = Scope::Venue("bsc_dex".into());
    for (name, val) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.min_pool_liquidity_usd", json!(100)),
        ("dex.depth_band_pct", json!(5)),
        ("dex.depth_hysteresis_pct", json!(0)),
        ("dex.route_max_pools_per_edge", json!(max_per_edge)),
    ] {
        c.settings
            .set(name, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    c
}

/// Пара V2 с резервами и своей комиссией 0,25 % (fee_pips = 2500). token0 < token1 по адресу.
fn pair(addr: Address, x: Address, y: Address, rx: u128, ry: u128) -> Entry {
    let f = families::make("pancakeswap_v2", "pancakeswap_v2").unwrap();
    let (t0, t1, r0, r1) = if x < y {
        (x, y, rx, ry)
    } else {
        (y, x, ry, rx)
    };
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(addr),
            token0: t0,
            token1: t1,
            fee_pips: Some(2500),
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::from(r0),
                reserve1: U256::from(r1),
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin: x,
        usdt: y,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

const BIG: Address = address!("0x00000000000000000000000000000000000000a1");
const SMALL: Address = address!("0x00000000000000000000000000000000000000a2");
const SHALLOW: Address = address!("0x00000000000000000000000000000000000000a3");
const CW: Address = address!("0x00000000000000000000000000000000000000a4");
const WU: Address = address!("0x00000000000000000000000000000000000000a5");

/// Граф из набора пулов: два глубоких COIN–USDT (разной глубины), мелкий COIN–USDT, цепочка COIN–WBNB–USDT.
async fn graph_with(max_per_edge: i64) -> graph::Graph {
    let p = Parts::new(ctx(max_per_edge).await);
    let e18 = 10u128.pow(18);
    p.tokens.set(&[
        Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        Asset {
            ticker: "X".into(),
            bsc_contract: Some(COIN),
        },
    ]);
    for e in [
        pair(BIG, COIN, USDT, 1_000_000 * e18, 1_000_000 * e18),
        pair(SMALL, COIN, USDT, 10_000 * e18, 10_000 * e18),
        pair(SHALLOW, COIN, USDT, 10 * e18, 10 * e18), // глубина ≈ 0,5 USDT < 100 — отсеивается
        pair(CW, COIN, WBNB, 100_000 * e18, 500 * e18),
        pair(WU, WBNB, USDT, 1_000 * e18, 600_000 * e18),
    ] {
        p.registry.put(e);
    }
    graph::build(&p.ctx, &p.registry, &p.tokens, 18).expect("граф строится")
}

fn pool_of(e: &graph::Edge) -> Address {
    match e.entry.pool.reference {
        PoolRef::Contract(a) => a,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn adjacency_and_shallow_filter() {
    let g = graph_with(2).await;
    // Три пары: COIN–USDT (два пула), COIN–WBNB, WBNB–USDT; мелкий COIN–USDT отсеян.
    assert_eq!(g.len(), 4, "рёбер: 2 COIN–USDT + COIN–WBNB + WBNB–USDT");
    let coin_usdt: Vec<_> = g.between(COIN, USDT).iter().map(|e| pool_of(e)).collect();
    assert_eq!(coin_usdt.len(), 2);
    assert!(coin_usdt.contains(&BIG) && coin_usdt.contains(&SMALL));
    assert!(!coin_usdt.contains(&SHALLOW), "мелкий пул не в графе");
    // Смежность: у USDT соседи COIN и WBNB; у COIN — USDT и WBNB.
    let nbrs = |t: Address| {
        let mut v: Vec<Address> = g.edges_from(t).map(|e| e.other(t)).collect();
        v.sort();
        v.dedup();
        v
    };
    assert!(nbrs(USDT).contains(&COIN) && nbrs(USDT).contains(&WBNB));
    assert!(nbrs(COIN).contains(&USDT) && nbrs(COIN).contains(&WBNB));
    assert_eq!(g.between(COIN, WBNB).len(), 1);
    assert_eq!(g.between(WBNB, USDT).len(), 1);
}

#[tokio::test]
async fn cap_keeps_deepest() {
    // Предел 1 на ребро: из двух глубоких COIN–USDT остаётся самый глубокий (BIG).
    let g = graph_with(1).await;
    let coin_usdt = g.between(COIN, USDT);
    assert_eq!(coin_usdt.len(), 1);
    assert_eq!(pool_of(coin_usdt[0]), BIG);
    assert_eq!(g.len(), 3, "по одному ребру на пару");
}
