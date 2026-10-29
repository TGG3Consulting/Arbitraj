//! Файл: tests/bsc_dex_route_paths.rs
//! Что делает: проверяет перечисление путей по графу (quote/route_paths.rs, решение 80, шаг 2) на РЕАЛЬНОЙ записи
//! маршрута CAKE 2026-10-29 (tests/data/routes_2026-10-29, рекордер record_route.js) — состояния пулов настоящие, на
//! одном блоке 130521896; шесть пулов: CAKE-USDT ×2 (Pancake V3 + Uniswap V3), CAKE-WBNB (Pancake V3),
//! WBNB-USDT (V2), CAKE-BTCB (Uniswap V3), BTCB-WBNB (Pancake V3). Проверяется: из USDT в CAKE находятся прямой путь
//! (оба пула пары), путь глубины 2 (через WBNB) и глубины 3 (через BTCB и WBNB); пути не заходят в токен дважды
//! (нет циклов, нет возврата в старт, решение 80 §6); ограничение ширины и набор разрешённых промежуточных работают.
//! Адреса токенов записью не захвачены (рекордер снимает только состояние пулов), поэтому узлы графа — условные
//! адреса в том же порядке сортировки, что реальные (CAKE < USDT < BTCB < WBNB), чтобы ориентация token0/token1 у V3
//! совпадала с записанным sqrtPriceX96; пары пулов — по тегам записи (авторитетная топология от техлида).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools, quote, parts}, abi_v2v3; настоящая SQLite во временной папке.

use std::collections::{BTreeMap, HashMap, HashSet};

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{Asset, Pool, PoolRef, PoolState, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::abi_v2v3::{
    IPancakeV3Pool, ITickLens, IUniswapV3Pool, IV2Pair, IV3Pool,
};
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::route_paths;

// Узлы графа: порядок сортировки — как у реальных адресов BSC (CAKE 0x0e.. < USDT 0x55.. < BTCB 0x71.. < WBNB 0xbb..),
// чтобы token0 < token1 у V3 совпадало с записанным sqrtPriceX96. Сами адреса условные (запись их не содержит).
const CAKE: Address = address!("0x0e00000000000000000000000000000000000001");
const USDT: Address = address!("0x5500000000000000000000000000000000000002");
const BTCB: Address = address!("0x7100000000000000000000000000000000000003");
const WBNB: Address = address!("0xbb00000000000000000000000000000000000004");

/// Записанные ответы: имя вызова → байты результата.
fn load() -> HashMap<String, Vec<u8>> {
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/routes_2026-10-29/responses.json").unwrap(),
    )
    .unwrap();
    let mut got = HashMap::new();
    for r in v["results"].as_array().unwrap() {
        if let Some(s) = r["response"]["result"].as_str() {
            got.insert(
                r["name"].as_str().unwrap().to_string(),
                alloy_primitives::hex::decode(s).unwrap(),
            );
        }
    }
    got
}

fn base(kind: &str, fee: Option<u32>, addr: Address, coin: Address, counter: Address) -> Entry {
    let f = families::make(kind, kind).unwrap();
    let (t0, t1) = if coin < counter {
        (coin, counter)
    } else {
        (counter, coin)
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
            fee_pips: fee,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::ZERO,
                reserve1: U256::ZERO,
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin,
        usdt: counter,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

/// Пул V3 из записи (slot0 + liquidity + слова TickLens) как запись реестра. pancake — ABI PancakeSwap (slot0 шире).
#[allow(clippy::too_many_arguments)]
fn v3(
    got: &HashMap<String, Vec<u8>>,
    tag: &str,
    pancake: bool,
    spacing: i32,
    fee: u32,
    words: (i16, i16),
    addr: Address,
    coin: Address,
    counter: Address,
) -> Entry {
    let (sqrt, tick) = if pancake {
        let s =
            IPancakeV3Pool::slot0Call::abi_decode_returns(&got[&format!("{tag} slot0")]).unwrap();
        (s.sqrtPriceX96.to::<U256>(), s.tick.as_i32())
    } else {
        let s =
            IUniswapV3Pool::slot0Call::abi_decode_returns(&got[&format!("{tag} slot0")]).unwrap();
        (s.sqrtPriceX96.to::<U256>(), s.tick.as_i32())
    };
    let liquidity =
        IV3Pool::liquidityCall::abi_decode_returns(&got[&format!("{tag} liquidity")]).unwrap();
    let mut ticks = BTreeMap::new();
    for k in words.0..=words.1 {
        for t in ITickLens::getPopulatedTicksInWordCall::abi_decode_returns(
            &got[&format!("{tag} TickLens word {k}")],
        )
        .unwrap()
        {
            ticks.insert(
                t.tick.as_i32(),
                TickData {
                    gross: t.liquidityGross,
                    net: t.liquidityNet,
                },
            );
        }
    }
    let kind = if pancake {
        "pancakeswap_v3"
    } else {
        "uniswap_v3"
    };
    let mut e = base(kind, Some(fee), addr, coin, counter);
    e.pool.tick_spacing = Some(spacing);
    e.pool.state = PoolState::V3 {
        sqrt_price_x96: sqrt,
        liquidity,
        tick,
    };
    e.ticks = Some(TickMap {
        spacing,
        ticks,
        words,
    });
    e
}

/// Пул V2 из записи (getReserves). Комиссия 0,25% (fee_pips 2500) — для rate.rs при пересчёте глубины в USDT.
fn v2(
    got: &HashMap<String, Vec<u8>>,
    tag: &str,
    addr: Address,
    coin: Address,
    counter: Address,
) -> Entry {
    let rs =
        IV2Pair::getReservesCall::abi_decode_returns(&got[&format!("{tag} getReserves")]).unwrap();
    // Узлы графа заданы в том же порядке сортировки, что реальные адреса, поэтому reserve0/reserve1 записи ложатся
    // на token0/token1 напрямую (как в реальном пуле).
    let mut e = base("pancakeswap_v2", Some(2500), addr, coin, counter);
    e.pool.state = PoolState::V2 {
        reserve0: U256::from(rs.reserve0),
        reserve1: U256::from(rs.reserve1),
    };
    e
}

async fn ctx() -> Ctx {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let uniq = format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let d = std::env::temp_dir().join(format!("arb-route-paths-{uniq}"));
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
    for (n, val) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.min_pool_liquidity_usd", json!(1)),
        ("dex.depth_band_pct", json!(5)),
        ("dex.depth_hysteresis_pct", json!(0)),
        ("dex.route_max_pools_per_edge", json!(5)),
        ("dex.route_max_hops", json!(3)),
        ("dex.route_max_paths", json!(50)),
    ] {
        c.settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    c
}

/// Граф из реальной записи. Монета — CAKE, промежуточные — WBNB и BTCB; встречный токен каждого ребра имеет прямой
/// пул с USDT (для пересчёта глубины, решение 73): у CAKE-BTCB встречный — CAKE, у *-WBNB — WBNB.
async fn graph_and_ctx() -> (graph::Graph, Parts) {
    let got = load();
    let p = Parts::new(ctx().await);
    p.tokens.set(&[Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(USDT),
    }]);
    for e in [
        v3(
            &got,
            "CAKE-USDT pcs_v3",
            true,
            50,
            2500,
            (-3, 3),
            address!("0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1"),
            CAKE,
            USDT,
        ),
        v3(
            &got,
            "CAKE-USDT uni_v3",
            false,
            60,
            3000,
            (-3, 3),
            address!("0xfe4fe5b4575c036ac6d5cccfe13660020270e27a"),
            CAKE,
            USDT,
        ),
        v3(
            &got,
            "CAKE-WBNB pcs_v3",
            true,
            50,
            2500,
            (-8, -2),
            address!("0x133b3d95bad5405d14d53473671200e9342896bf"),
            CAKE,
            WBNB,
        ),
        v2(
            &got,
            "WBNB-USDT v2",
            address!("0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae"),
            WBNB,
            USDT,
        ),
        v3(
            &got,
            "CAKE-BTCB uni_v3",
            false,
            200,
            10000,
            (-3, 3),
            address!("0x8e8410f4878079fc03a287a7ded4a65afb87cc06"),
            BTCB,
            CAKE,
        ),
        v3(
            &got,
            "BTCB-WBNB pcs_v3",
            true,
            10,
            500,
            (15, 21),
            address!("0x6bbc40579ad1bbd243895ca0acb086bb6300d636"),
            BTCB,
            WBNB,
        ),
    ] {
        p.registry.put(e);
    }
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 18).expect("граф строится");
    (g, p)
}

/// Последовательность токенов пути: вход первого хопа и выходы по порядку.
fn seq(path: &route_paths::RoutePath) -> Vec<Address> {
    let mut v = vec![path.steps[0].token_in];
    v.extend(path.steps.iter().map(|s| s.token_out));
    v
}

#[tokio::test]
async fn enumerates_direct_two_three_hops() {
    let (g, _p) = graph_and_ctx().await;
    assert_eq!(g.len(), 6, "все шесть пулов записи — в графе");
    let allowed = HashSet::from([WBNB, BTCB]);
    let paths = route_paths::enumerate(&g, USDT, CAKE, &allowed, 3, 50);

    // Прямой (2 пула пары) + глубины 2 (через WBNB) + глубины 3 (через BTCB и WBNB).
    let by_hops = |h: usize| paths.iter().filter(|p| p.hops() == h).count();
    assert_eq!(by_hops(1), 2, "два прямых пула CAKE-USDT");
    assert_eq!(by_hops(2), 1, "путь глубины 2");
    assert_eq!(by_hops(3), 1, "путь глубины 3");
    assert_eq!(paths.len(), 4);

    let two = paths.iter().find(|p| p.hops() == 2).unwrap();
    assert_eq!(seq(two), vec![USDT, WBNB, CAKE]);
    let three = paths.iter().find(|p| p.hops() == 3).unwrap();
    assert_eq!(seq(three), vec![USDT, WBNB, BTCB, CAKE]);

    // Ни один путь не заходит в токен дважды (нет циклов, нет возврата в старт — решение 80 §6).
    for p in &paths {
        let s = seq(p);
        let uniq: HashSet<_> = s.iter().collect();
        assert_eq!(uniq.len(), s.len(), "путь без повторного захода в токен");
        assert_eq!(*s.first().unwrap(), USDT);
        assert_eq!(*s.last().unwrap(), CAKE);
    }
}

#[tokio::test]
async fn width_and_allowed_limits() {
    let (g, _p) = graph_and_ctx().await;
    // Без разрешённых промежуточных — только прямые пути.
    let direct = route_paths::enumerate(&g, USDT, CAKE, &HashSet::new(), 3, 50);
    assert_eq!(direct.len(), 2);
    assert!(direct.iter().all(|p| p.hops() == 1));
    // Предел хопов 2 — путь глубины 3 отсечён.
    let allowed = HashSet::from([WBNB, BTCB]);
    let hops2 = route_paths::enumerate(&g, USDT, CAKE, &allowed, 2, 50);
    assert!(hops2.iter().all(|p| p.hops() <= 2));
    assert_eq!(hops2.iter().filter(|p| p.hops() == 2).count(), 1);
    // Ширина 1 — не больше одного пути.
    assert_eq!(
        route_paths::enumerate(&g, USDT, CAKE, &allowed, 3, 1).len(),
        1
    );
}

#[tokio::test]
async fn limits_read_from_settings() {
    let (_g, p) = graph_and_ctx().await;
    assert_eq!(route_paths::limits(&p.ctx).unwrap(), (3, 50));
}
