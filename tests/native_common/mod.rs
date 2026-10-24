//! Файл: tests/native_common/mod.rs
//! Что делает: общее для тестов нативной монеты BNB (решение 62): окружение на настоящей SQLite с настройками строк DEX,
//! пул по виду, план сделки тем же кодом, что исполнение (exec/native.rs + exec/plan.rs), точные вызовы узла для записи
//! (tests/data/native_calls_2026-10-24.json — PancakeSwap UR + V2; calls2.rs — V3 через PancakeSwap и Uniswap UR) и
//! разбор записанных ответов.
//! Кто вызывает: tests/bsc_dex_native.rs, tests/bsc_dex_native_live.rs.
//! От чего зависит: arb::venues::bsc_dex::{exec, pools, node}, arb::settings, arb::storage.
//! Адреса BSC: WBNB — weth9 в параметрах развёртывания Universal Router (PancakeSwap DeployBsc.s.sol#L24, Uniswap
//! DeployBSC.s.sol#L11); USDT — как у BingX в сети BSC; Universal Router PancakeSwap — tx_to настоящих обменов в
//! tests/data/indexers_2026-10-24/arb_node_responses_2_2026-10-24.json; фабрики V3 PancakeSwap и Uniswap — ответы
//! factory() пулов CAKE в tests/data/indexers_2026-10-24/arb_node_responses_2026-10-24.json; Universal Router Uniswap
//! в BSC — официальная страница развёртываний Uniswap для BNB Chain (UniversalRouter, Permit2, UniswapV3Factory):
//! https://developers.uniswap.org/docs/protocols/v3/deployments/v3-bnb-deployments ; код и симуляция — запись
//! tests/data/native_2026-10-25/arb_native_responses_2_2026-10-25.json. В коде проекта адресов нет — только настройки.
#![allow(dead_code, unused_imports)]

pub mod calls1;
pub mod calls2;
pub mod record;

pub use calls1::calls;
pub use record::{answer, decode, receipt, usdt_to};

use alloy_primitives::{Address, Bytes, U256, address};
use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{Pool, PoolRef, PoolState, Side, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::native;
use arb::venues::bsc_dex::exec::plan::{self, Order, Plan};
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;

pub const WBNB: Address = address!("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
pub const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");
pub const PCS_UR: Address = address!("0xd9C500DfF816a1Da21A48A732d3498Bf09dc9AEB");
pub const UNI_UR: Address = address!("0x1906c1d672b88cd1b9ac7593301ca990f94eae07");
pub const PCS_V2_FACTORY: Address = address!("0xcA143Ce32Fe78f1f7019d7d551a6402fC5350c73");
pub const PCS_V2_ROUTER: Address = address!("0x10ED43C718714eb63d5aA57B78B54704E256024E");
pub const PCS_V3_FACTORY: Address = address!("0x0bfbcf9fa4f9c56b0f40a671ad40e0805a091865");
pub const UNI_V3_FACTORY: Address = address!("0xdb1d10011ad0ff90774d0c6bb92e5c5c8b4461f7");
/// Кошелёк для записи симуляции: горячий кошелёк Binance в BSC (BNB и USDT хватило — запись 2026-10-25).
pub const HOLDER: Address = address!("0x8894E0a0c962CB723c1976a4421c95949bE2D4E3");
/// Срок execute для записи: блок времени + dex.deadline_s (60) = 2 000 000 000 (2033 г.).
pub const NOW: u64 = 1_999_999_940;
pub const CALLS: &str = "tests/data/native_calls_2026-10-24.json";
pub const CALLS2: &str = "tests/data/native_calls_2_2026-10-25.json";

/// Строки DEX видов V3: PancakeSwap (UR PancakeSwap Infinity) и Uniswap (UR Uniswap 2.0). Permit2 в плане нативной
/// монеты не нужен (платит роутер), но настройка строки обязательна: PancakeSwap — как у V2, Uniswap — канонический.
fn v3_rows() -> Vec<(&'static str, Scope, Value)> {
    let p = Scope::Dex("pancakeswap_v3".into());
    let u = Scope::Dex("uniswap_v3".into());
    vec![
        ("dex.kind", p.clone(), json!("pancakeswap_v3")),
        ("dex.universal_router", p.clone(), json!(PCS_UR.to_string())),
        (
            "dex.universal_router_version",
            p.clone(),
            json!("pancakeswap_infinity"),
        ),
        (
            "dex.permit2",
            p,
            json!("0x31c2F6fcFf4F8759b3Bd5Bf0e1084A055615c768"),
        ),
        ("dex.kind", u.clone(), json!("uniswap_v3")),
        ("dex.universal_router", u.clone(), json!(UNI_UR.to_string())),
        ("dex.universal_router_version", u.clone(), json!("2.0")),
        (
            "dex.permit2",
            u,
            json!("0x000000000022D473030F116dDEE9F6B43aC78BA3"),
        ),
    ]
}

/// Окружение на настоящей SQLite: настройки нативной монеты и строк DEX pancakeswap_v2, pancakeswap_v3, uniswap_v3.
pub async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-native-{tag}-{}", std::process::id()));
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
    let s = Scope::Dex("pancakeswap_v2".into());
    for (name, scope, val) in v3_rows().into_iter().chain([
        ("dex.native_ticker", v.clone(), json!("BNB")),
        ("dex.native_wrapped", v.clone(), json!(WBNB.to_string())),
        ("dex.deadline_s", v, json!(60)),
        ("dex.kind", s.clone(), json!("pancakeswap_v2")),
        ("dex.universal_router", s.clone(), json!(PCS_UR.to_string())),
        (
            "dex.universal_router_version",
            s.clone(),
            json!("pancakeswap_infinity"),
        ),
        // Permit2 в плане нативной монеты не нужен (платит роутер), но настройка строки обязательна.
        (
            "dex.permit2",
            s,
            json!("0x31c2F6fcFf4F8759b3Bd5Bf0e1084A055615c768"),
        ),
    ]) {
        c.settings
            .set(name, scope, Some(val), "тест")
            .await
            .unwrap();
    }
    c
}

/// Пул вида `kind` с комиссией V3 (pips) или без неё. Адрес пула в данные вызова не входит: путь V2 — токены, V3 —
/// токены и комиссия (Universal Router находит пул по своей фабрике); проверка «кто меняет» — по записи.
pub fn entry(kind: &str, fee: Option<u32>) -> Entry {
    let f = families::make(kind, kind).unwrap();
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(Address::ZERO),
            token0: USDT,
            token1: WBNB,
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
        ticker: "BNB".into(),
        coin: WBNB,
        usdt: USDT,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

/// План продажи (WRAP_ETH) или покупки (UNWRAP_WETH) BNB в одном пуле — тем же кодом, что сделка (exec/native.rs::adapt).
pub async fn native_plan(c: &Ctx, e: &Entry, side: Side, amount: U256, wallet: Address) -> Plan {
    let (tin, tout) = if side == Side::Sell {
        (WBNB, USDT)
    } else {
        (USDT, WBNB)
    };
    let o = Order {
        token_in: tin,
        token_out: tout,
        amount_in: amount,
        min_out: U256::from(1u8),
    };
    let mut legs = vec![plan::single(e, o)];
    let w = native::adapt(c, side, &mut legs).unwrap();
    if side == Side::Buy {
        // Для записи без Permit2: USDT заранее на роутере (вызов transfer до execute) — платит роутер.
        legs[0][0].from_wallet = false;
    }
    plan::build(c, &legs[0], wallet, NOW, true)
        .await
        .unwrap()
        .with_native(w)
}
