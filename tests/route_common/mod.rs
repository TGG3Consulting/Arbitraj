//! Файл: tests/route_common/mod.rs
//! Что делает: общее для тестов графа-маршрутизатора (решение 80) на записях маршрута CAKE (рекордер record_route.js):
//! routes_2026-10-29 (блок 130521896) и routes_2026-10-30 (блок 130556914) по тем же шести пулам — разбор состояния
//! шести пулов (V2 + Pancake/Uniswap V3) в записи реестра бота и окружение на настоящей SQLite со всеми настройками и
//! проверками, при которых пул допущен к котировке (quote/select.rs): строки DEX, глубина (решение 73), сверка с
//! квотером и газ пробы, сплит, граф. Состояния пулов настоящие, каждая запись — на одном блоке; две записи дают
//! проверку дрейфа цены для предторговой верификации (quote/verify.rs, решение 83). Адреса токенов записью не
//! захвачены, поэтому узлы графа — условные адреса в том же порядке сортировки, что реальные
//! (CAKE < USDT < BTCB < WBNB), чтобы ориентация token0/token1 у V3 совпадала с записанным sqrtPriceX96.
//! Пары пулов — по тегам записи (авторитетная топология).
//! Кто вызывает: tests/bsc_dex_split.rs, tests/bsc_dex_verify.rs.
//! От чего зависит: arb::venues::bsc_dex::{pools, parts, ctx}, abi_v2v3; arb::settings; arb::storage.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{
    Asset, CheckState, CoinCheck, Pool, PoolRef, PoolState, TokenTaxes, VenueId,
};
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
use arb::venues::bsc_dex::pools::mids::Mid;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

pub const CAKE: Address = address!("0x0e00000000000000000000000000000000000001");
pub const USDT: Address = address!("0x5500000000000000000000000000000000000002");
pub const BTCB: Address = address!("0x7100000000000000000000000000000000000003");
pub const WBNB: Address = address!("0xbb00000000000000000000000000000000000004");

pub const CAKE_USDT_PCS: Address = address!("0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1");
pub const CAKE_USDT_UNI: Address = address!("0xfe4fe5b4575c036ac6d5cccfe13660020270e27a");
pub const CAKE_WBNB_PCS: Address = address!("0x133b3d95bad5405d14d53473671200e9342896bf");
pub const WBNB_USDT_V2: Address = address!("0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae");
pub const CAKE_BTCB_UNI: Address = address!("0x8e8410f4878079fc03a287a7ded4a65afb87cc06");
pub const BTCB_WBNB_PCS: Address = address!("0x6bbc40579ad1bbd243895ca0acb086bb6300d636");

fn load(dir: &str) -> HashMap<String, Vec<u8>> {
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}/responses.json")).unwrap())
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
        ticker: "CAKE".into(),
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

fn v2(
    got: &HashMap<String, Vec<u8>>,
    tag: &str,
    addr: Address,
    coin: Address,
    counter: Address,
) -> Entry {
    let rs =
        IV2Pair::getReservesCall::abi_decode_returns(&got[&format!("{tag} getReserves")]).unwrap();
    let mut e = base("pancakeswap_v2", Some(2500), addr, coin, counter);
    // Узлы в порядке сортировки реальных адресов — reserve0/reserve1 ложатся на token0/token1 напрямую.
    e.pool.state = PoolState::V2 {
        reserve0: U256::from(rs.reserve0),
        reserve1: U256::from(rs.reserve1),
    };
    e
}

/// Строки настроек одного вида DEX, без которых пул «не подключён» (pools/families.rs::required). Адреса условные:
/// classify проверяет только НАЛИЧИЕ, не значение (к узлу тест не ходит).
fn dex_row(id: &str, version: &str, v3: bool) -> Vec<(String, Scope, Value)> {
    let s = Scope::Dex(id.into());
    let a = json!("0x0000000000000000000000000000000000000099");
    let mut out = vec![
        ("dex.kind", json!(id)),
        ("dex.pool_enabled", json!(true)),
        ("dex.universal_router", a.clone()),
        ("dex.universal_router_version", json!(version)),
        ("dex.permit2", a.clone()),
        ("dex.factory", a.clone()),
    ];
    if v3 {
        out.push(("dex.tick_lens", a.clone()));
        out.push(("dex.quoter", a.clone()));
    } else {
        out.push(("dex.router", a.clone()));
        out.push(("dex.v2_fee_source", json!("fixed")));
        out.push(("dex.v2_fee_pct", json!("0.25")));
    }
    out.into_iter()
        .map(|(n, v)| (n.to_string(), s.clone(), v))
        .collect()
}

async fn ctx() -> Ctx {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let uniq = format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let d = std::env::temp_dir().join(format!("arb-route-common-{uniq}"));
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
    let venue: Vec<(String, Scope, Value)> = [
        ("venue.quote_asset", json!("USDT")),
        ("dex.min_pool_liquidity_usd", json!(1)),
        ("dex.depth_band_pct", json!(5)),
        ("dex.depth_hysteresis_pct", json!(0)),
        ("dex.route_max_pools_per_edge", json!(5)),
        ("dex.route_max_hops", json!(3)),
        ("dex.route_max_paths", json!(50)),
        ("dex.intermediate_enabled", json!(true)),
        ("smart.split_step_pct", json!(10)),
        ("smart.max_splits", json!(5)),
    ]
    .into_iter()
    .map(|(n, val)| (n.to_string(), v.clone(), val))
    .collect();
    for (name, scope, val) in venue
        .into_iter()
        .chain(dex_row("pancakeswap_v3", "pancakeswap_infinity", true))
        .chain(dex_row("uniswap_v3", "2.0", true))
        .chain(dex_row("pancakeswap_v2", "pancakeswap_infinity", false))
    {
        c.settings
            .set(&name, scope, Some(val), "тест")
            .await
            .unwrap();
    }
    c
}

/// Окружение по записи маршрута routes_2026-10-29 (блок 130521896) — удобный частный случай parts_from.
pub async fn parts() -> Parts {
    parts_from("tests/data/routes_2026-10-29").await
}

/// Окружение с шестью пулами записи из каталога `dir` (responses.json рекордера record_route.js), заданными decimals,
/// промежуточными (WBNB, BTCB) и проверками (газ + сверка с квотером по обеим сторонам), при которых пулы допущены к
/// котировке. Разные каталоги = разные блоки по тем же пулам (проверка дрейфа для verify.rs).
pub async fn parts_from(dir: &str) -> Parts {
    let got = load(dir);
    let p = Parts::new(ctx().await);
    p.tokens.set(&[
        Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE),
        },
    ]);
    for t in [CAKE, USDT, BTCB, WBNB] {
        p.tokens.remember_decimals(t, 18);
    }
    // Налоги CAKE проверены и нулевые — нужно классике (direct_route → direct::taxes); граф берёт налоги из Via.
    p.checks.set_coin(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: Some(CAKE),
        },
        taxes: CheckState::Known(TokenTaxes {
            buy_pct: Decimal::ZERO,
            sell_pct: Decimal::ZERO,
            transfer_pct: Decimal::ZERO,
        }),
        source: "тест".into(),
        checked_at: Some(Utc::now()),
        wait: None,
    });
    p.mids.set(vec![
        Mid {
            address: WBNB,
            ticker: "BNB".into(),
        },
        Mid {
            address: BTCB,
            ticker: "BTCB".into(),
        },
    ]);
    let pools = [
        v3(
            &got,
            "CAKE-USDT pcs_v3",
            true,
            50,
            2500,
            (-3, 3),
            CAKE_USDT_PCS,
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
            CAKE_USDT_UNI,
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
            CAKE_WBNB_PCS,
            CAKE,
            WBNB,
        ),
        v2(&got, "WBNB-USDT v2", WBNB_USDT_V2, WBNB, USDT),
        v3(
            &got,
            "CAKE-BTCB uni_v3",
            false,
            200,
            10000,
            (-3, 3),
            CAKE_BTCB_UNI,
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
            BTCB_WBNB_PCS,
            BTCB,
            WBNB,
        ),
    ];
    for e in pools {
        let r = e.pool.reference.clone();
        p.registry.put(e);
        p.checks.update_pool(&r, |f| {
            f.gas_buy = Some(120_000);
            f.gas_sell = Some(120_000);
            f.quoter.buy = Some(Ok(()));
            f.quoter.sell = Some(Ok(()));
        });
    }
    p
}
