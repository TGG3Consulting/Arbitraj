//! Файл: tests/bsc_dex_inf_stable_answers.rs
//! Что делает: ответы Тиграна по Infinity Stable (решение «122 (Infinity Stable, ответы)») без сети и без моков, на
//! настоящей SQLite настроек: (1) крючок из dex.stable_hooks ЗАНЯТ — вид Infinity CL такой пул не берёт (поиск по
//! Initialize даёт None, обычный крючок — пул); (2) основной квотер Infinity Stable — CLQuoter (dex.quoter,
//! quoteExactInputSingle с ключом пула), дополнительный — get_dy крючка с индексами по токену входа; у других видов
//! дополнительного нет.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (pools::{families, discover_v4, verify_inf_stable, key, abi_v4,
//! registry}, quote::{quoter_calls, quoter_extra}), arb::settings, arb::storage.

use alloy::sol_types::{SolCall, SolEvent};
use alloy_primitives::aliases::{I24, U24, U160};
use alloy_primitives::{Address, B256, U256, address};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{Pool, PoolRef, PoolState, StableData, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::{db::Db, journal::Journal, raw_log::RawLog};
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::node::abi_dex::IInfinityQuoter;
use arb::venues::bsc_dex::pools::abi_inf_stable::IInfStableHook as H;
use arb::venues::bsc_dex::pools::abi_v4::IInfinityCl;
use arb::venues::bsc_dex::pools::key::{InfinityPoolKeyData, PoolKey};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::{discover_v4, families, verify_inf_stable};
use arb::venues::bsc_dex::quote::{quoter_calls, quoter_extra};

const HOOK: Address = address!("0x86c3DC08FB5a6663cCa15551575d5429e5Efc017");
const OTHER: Address = address!("0x2222222222222222222222222222222222222222");
const MANAGER: Address = address!("0x3333333333333333333333333333333333333333");
const QUOTER: Address = address!("0x4444444444444444444444444444444444444444");
const C0: Address = address!("0x55d398326f99059fF775485246999027B3197955");
const C1: Address = address!("0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d");

/// Окружение на настоящей SQLite: вид Infinity Stable (id «inf-stable») со списком крючков [HOOK] и квотером.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-infst-{tag}-{}", std::process::id()));
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
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    };
    let s = Scope::Dex("inf-stable".into());
    let list: [(&str, Value); 3] = [
        ("dex.kind", json!("pancake_infinity_stable")),
        ("dex.stable_hooks", json!([format!("{HOOK:#x}")])),
        ("dex.quoter", json!(format!("{QUOTER:#x}"))),
    ];
    for (n, v) in list {
        c.settings.set(n, s.clone(), Some(v), "тест").await.unwrap();
    }
    c
}

/// Ключ пула с крючком: флаги 0x0455 (как getHooksRegistrationBitmap крючка), tickSpacing 1 в битах 16+.
fn key(hooks: Address) -> InfinityPoolKeyData {
    let mut p = [0u8; 32];
    p[29] = 1; // tickSpacing = 1 << 16
    p[30] = 0x04;
    p[31] = 0x55;
    InfinityPoolKeyData {
        currency0: C0.min(C1),
        currency1: C0.max(C1),
        hooks,
        manager: MANAGER,
        fee: 100,
        parameters: B256::from(p),
    }
}

/// Лог Initialize CLPoolManager по ключу (PoolId — keccak ключа).
fn initialize(k: &InfinityPoolKeyData) -> alloy::rpc::types::Log {
    let id = PoolKey::Infinity(k.clone()).id();
    let ev = IInfinityCl::Initialize {
        id,
        currency0: k.currency0,
        currency1: k.currency1,
        hooks: k.hooks,
        fee: U24::from(k.fee),
        parameters: k.parameters,
        sqrtPriceX96: U160::from(1u128 << 96),
        tick: I24::ZERO,
    };
    alloy::rpc::types::Log {
        inner: alloy_primitives::Log {
            address: MANAGER,
            data: ev.encode_log_data(),
        },
        ..Default::default()
    }
}

/// (1) Один пул — одна математика: занятый крючок — только Infinity Stable.
#[tokio::test]
async fn stable_hook_is_not_infinity_cl() {
    let c = ctx("claim").await;
    assert!(verify_inf_stable::claimed(&c, HOOK));
    assert!(!verify_inf_stable::claimed(&c, OTHER));
    let cl = families::make("inf-cl", "pancakeswap_infinity_cl").unwrap();
    let st = families::make("inf-stable", "pancake_infinity_stable").unwrap();
    // Initialize с занятым крючком: CL — нет пула, Infinity Stable — пул (ссылка — крючок, ключ сохранён).
    let log = initialize(&key(HOOK));
    assert!(discover_v4::parse_opt(&c, cl, &log).unwrap().is_none());
    let f = discover_v4::parse_opt(&c, st, &log)
        .unwrap()
        .expect("пул Infinity Stable");
    assert_eq!(f.reference, PoolRef::Contract(HOOK));
    assert!(f.hooks.is_none() && f.key.is_some());
    // Чужой крючок: CL берёт, Infinity Stable — нет.
    let log = initialize(&key(OTHER));
    assert!(discover_v4::parse_opt(&c, cl, &log).unwrap().is_some());
    assert!(discover_v4::parse_opt(&c, st, &log).unwrap().is_none());
}

/// Запись реестра пула Infinity Stable с ключом.
fn entry(kind: &str) -> Entry {
    let f = families::make("inf-stable", kind).unwrap();
    let k = key(HOOK);
    let state = StableData {
        balances: vec![U256::from(10u64.pow(18)); 2],
        rates: vec![U256::from(10u64.pow(18)); 2],
        amp: U256::from(20_000u64),
        fee: U256::from(1_000_000u64),
        amp_precision: U256::from(100u8),
        max_rounds: 255,
        strict: true,
        guard: None,
        halted: false,
        i0: 0,
        i1: 1,
        ng: None,
        native: None,
    };
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(HOOK),
            token0: k.currency0,
            token1: k.currency1,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::StableSwap(Box::new(state)),
            block: 1,
            received_at: chrono::Utc::now(),
        },
        ticker: "USDC".into(),
        coin: C1,
        usdt: C0,
        key: Some(PoolKey::Infinity(k)),
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

/// (2) Основной квотер — CLQuoter (dex.quoter), дополнительный — get_dy крючка (индексы по токену входа).
#[tokio::test]
async fn quoters_main_cl_and_extra_get_dy() {
    let c = ctx("quoters").await;
    let e = entry("pancake_infinity_stable");
    let (t0, t1) = (e.pool.token0, e.pool.token1);
    let dx = U256::from(1_000u64);
    let (to, data) = quoter_calls::request(&c, &e, t1, dx).unwrap();
    assert_eq!(to, QUOTER);
    let q = IInfinityQuoter::quoteExactInputSingleCall::abi_decode(&data).unwrap();
    assert_eq!(q.params.poolKey.hooks, HOOK);
    assert!(!q.params.zeroForOne);
    let (to, data) = quoter_extra::request(&e, t0, dx).expect("get_dy");
    assert_eq!(to, HOOK);
    let g = H::get_dyCall::abi_decode(&data).unwrap();
    assert_eq!((g.i, g.j, g.dx), (0, 1, dx));
    let g = H::get_dyCall::abi_decode(&quoter_extra::request(&e, t1, dx).unwrap().1).unwrap();
    assert_eq!((g.i, g.j), (1, 0));
    // У других видов дополнительного квотера нет.
    assert!(quoter_extra::request(&entry("pancake_stableswap"), t0, dx).is_none());
}
