//! Файл: tests/tick_band_common/mod.rs
//! Что делает: общее для тестов решения 296 (полоса чтения карты тиков dex.tick_read_band_pct): окружение на настоящей
//! SQLite с настройками чтения карты; текущий тик и tickSpacing каждого пула V3 / Infinity CL / Uniswap V4 живой записи
//! 2026-10-25 (tests/data/depth_2026-10-25/responses.json, рекордер tests/recorders/record_depth.js); пул PancakeSwap V3
//! AAVE/USDT 0x2e31285a… (tickSpacing 1, слова битмапа ±8 вокруг текущего) как запись реестра бота и сужение его карты
//! до заданных слов — ровно то, что бот прочитал бы TickLens по этим словам (ответ TickLens — по одному слову).
//! Кто вызывает: tests/bsc_dex_tick_read_band.rs, tests/bsc_dex_tick_read_band_swap.rs,
//! tests/bsc_dex_tick_read_band_algebra.rs (решение 319), tests/bsc_dex_off_map_algebra.rs (решение 320).
//! От чего зависит: tests/depth_common (rec), tests/native_common (entry), arb::settings, arb::venues::bsc_dex.
//! Настройки: задаёт тест (значения — в вызовах set).
// Каждый тест берёт часть общего — остальное для него не используется.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};
use serde_json::Value;
use tokio::sync::broadcast;

use arb::core::types::PoolState;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::abi_v2v3::{IPancakeV3Pool, ITickLens, IUniswapV3Pool, IV3Pool};
use arb::venues::bsc_dex::pools::abi_v4::{IInfinityCl, IStateView};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};

use super::depth_common::{Rec, USDT};
use super::native_common;

/// Окружение на настоящей SQLite без настроек. Принимает: метку папки. Ошибок нет (тест падает).
pub async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-tick-band-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        chains: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

/// Записать (Some) или снять (None) настройку площадки bsc_dex. Ошибок нет (тест падает).
pub async fn set(c: &Ctx, name: &str, v: Option<Value>) {
    let scope = Scope::Venue("bsc_dex".into());
    c.settings.set(name, scope, v, "тест").await.unwrap();
}

/// Пулы записи с картой тиков: (метка, tickSpacing, текущий тик, записанные слова). Тик — из slot0 / getSlot0 своего
/// вида пула. Ошибок нет (запись — по форме).
pub fn ticked(r: &Rec) -> Vec<(String, i32, i32, (i16, i16))> {
    r.pools
        .iter()
        .filter(|p| p["spacing"].is_i64())
        .map(|p| {
            let tag = p["tag"].as_str().unwrap();
            let tick = match p["kind"].as_str().unwrap() {
                "pcs_v3" => {
                    IPancakeV3Pool::slot0Call::abi_decode_returns(&r.got[&format!("{tag} slot0")])
                        .unwrap()
                        .tick
                        .as_i32()
                }
                "uni_v3" => {
                    IUniswapV3Pool::slot0Call::abi_decode_returns(&r.got[&format!("{tag} slot0")])
                        .unwrap()
                        .tick
                        .as_i32()
                }
                "infinity_cl" => IInfinityCl::getSlot0Call::abi_decode_returns(
                    &r.got[&format!("{tag} CLPoolManager getSlot0")],
                )
                .unwrap()
                .tick
                .as_i32(),
                _ => IStateView::getSlot0Call::abi_decode_returns(
                    &r.got[&format!("{tag} StateView getSlot0")],
                )
                .unwrap()
                .tick
                .as_i32(),
            };
            let w = p["words"].as_array().unwrap();
            let words = (w[0].as_i64().unwrap() as i16, w[1].as_i64().unwrap() as i16);
            (
                tag.to_string(),
                p["spacing"].as_i64().unwrap() as i32,
                tick,
                words,
            )
        })
        .collect()
}

/// Адрес пула PancakeSwap V3 AAVE/USDT с tickSpacing 1 в записи (pools[].addr).
pub const AAVE_V3_TS1: &str = "0x2e31285ade0e055036e709fc4f1b2c56c6e8fc0f";
/// Адрес AAVE — tests/data/depth_2026-10-25/calls.json (coins.AAVE).
const AAVE: &str = "0xfb6115445bff7b52feb98650c87f44907e58f802";
/// fee() пула рекордер не читал: tickSpacing 1 у PancakeSwap V3 — уровень 100 (PancakeV3Factory, конструктор:
/// feeAmountTickSpacing[100] = 1). Комиссия одна и та же у всех сравниваемых карт — сравнение от неё не зависит.
const FEE_TS1: u32 = 100;

/// Пул AAVE/USDT PancakeSwap V3 (tickSpacing 1) с картой из записанных слов (±8). Ошибок нет (запись — по форме).
pub fn aave_ts1(r: &Rec) -> Entry {
    let p = r.pools.iter().find(|p| p["addr"] == AAVE_V3_TS1).unwrap();
    let tag = p["tag"].as_str().unwrap();
    let s = IPancakeV3Pool::slot0Call::abi_decode_returns(&r.got[&format!("{tag} slot0")]).unwrap();
    let liquidity =
        IV3Pool::liquidityCall::abi_decode_returns(&r.got[&format!("{tag} liquidity")]).unwrap();
    let spacing = p["spacing"].as_i64().unwrap() as i32;
    assert_eq!(spacing, 1);
    let w = p["words"].as_array().unwrap();
    let words = (w[0].as_i64().unwrap() as i16, w[1].as_i64().unwrap() as i16);
    let mut ticks = BTreeMap::new();
    for k in words.0..=words.1 {
        let out = &r.got[&format!("{tag} TickLens word {k}")];
        for t in ITickLens::getPopulatedTicksInWordCall::abi_decode_returns(out).unwrap() {
            let d = TickData {
                gross: t.liquidityGross,
                net: t.liquidityNet,
            };
            ticks.insert(t.tick.as_i32(), d);
        }
    }
    let (aave, usdt) = (
        Address::from_str(AAVE).unwrap(),
        Address::from_str(USDT).unwrap(),
    );
    let (t0, t1) = if aave < usdt {
        (aave, usdt)
    } else {
        (usdt, aave)
    };
    let mut e = native_common::entry("pancakeswap_v3", Some(FEE_TS1));
    e.pool.token0 = t0;
    e.pool.token1 = t1;
    e.pool.tick_spacing = Some(spacing);
    e.pool.state = PoolState::V3 {
        sqrt_price_x96: s.sqrtPriceX96.to::<U256>(),
        liquidity,
        tick: s.tick.as_i32(),
    };
    e.ticker = "AAVE".into();
    e.coin = aave;
    e.usdt = usdt;
    e.ticks = Some(TickMap {
        spacing,
        ticks: ticks.into(),
        words,
    });
    e
}

/// Та же запись с картой только по словам `words` (подмножество записанных): тики вне слов отброшены. Ошибок нет.
pub fn narrowed(e: &Entry, words: (i16, i16)) -> Entry {
    let mut out = e.clone();
    let map = e.ticks.as_ref().unwrap();
    assert!(
        words.0 >= map.words.0 && words.1 <= map.words.1,
        "слова {words:?} вне записанных {:?}",
        map.words
    );
    let ticks: BTreeMap<i32, TickData> = map
        .ticks
        .iter()
        .filter(|(t, _)| {
            let w = TickMap::word_of(**t, map.spacing);
            w >= words.0 && w <= words.1
        })
        .map(|(t, d)| (*t, *d))
        .collect();
    out.ticks = Some(TickMap {
        spacing: map.spacing,
        ticks: ticks.into(),
        words,
    });
    out
}
