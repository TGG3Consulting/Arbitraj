//! Файл: tests/core_mid_wake_349.rs
//! Что делает: решение 349 — событие пула промежуточной пары будит монеты, чьи маршруты идут через этот пул
//! (core/mid_wake.rs). Проверяет:
//! (1) поиск достижимых промежуточных (reach) — по шагам «промежуточная — промежуточная», без повторов (адреса — настоящие
//!     контракты BSC WBNB, USDT, USDC, BTCB, CAKE; связи заданы списком);
//! (2) wake целиком на настоящем ядре (SQLite во временной папке) с пулами записи routes_2026-10-29 (route_common:
//!     CAKE-USDT ×2, CAKE-WBNB, WBNB-USDT, CAKE-BTCB, BTCB-WBNB) в состоянии рынка ядра и справочником монет: событие
//!     WBNB-USDT и BTCB-WBNB будит CAKE (маршруты CAKE → WBNB → USDT и CAKE → BTCB → WBNB → USDT) и сбрасывает её память
//!     котировок (поколение quote_cache растёт); монета только с пулом к USDT (ONLY, пул записи с её адресом) не будится;
//!     пул без промежуточной (CAKE-USDT) — никого; промежуточные выключены — никого; классика (0 шагов) — CAKE будится
//!     событием пула её промежуточной.
//! Сверка с перечнем путей адаптера на записанном графе — tests/bsc_dex_route_paths.rs (mid_wake_covers_every_path);
//! слияние событий одной монеты в один пересчёт — план tick_plan.rs (tests/core_tick_plan_322.rs).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (mid_wake, market_state, ticker_index, quote_cache, engine), tests/route_common.
//! Моков нет: площадок в реестре шлюза нет — проверяется решение ядра.

mod route_common;

use alloy_primitives::{Address, address};
use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::engine::{lk, wr};
use arb::core::mid_wake::{self, reach};
use arb::core::registry::CoinEntry;
use arb::core::tick_plan::PoolMark;
use arb::core::types::{Asset, VenueId};
use arb::gateway::events::MarketEvent;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::broadcast;

const R_WBNB: Address = address!("bb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
const R_USDT: Address = address!("55d398326f99059fF775485246999027B3197955");
const R_USDC: Address = address!("8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d");
const R_BTCB: Address = address!("7130d2A12B9BCbFAe4f2634d864A1Ee1Ce3Ead9c");
const R_CAKE: Address = address!("0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");
/// Монета только с пулом к USDT (настоящий адрес CAKE — в записи маршрута CAKE условный).
const ONLY: Address = R_CAKE;

/// Соседи по пулам: WBNB — USDT, CAKE, USDC; USDC — USDT, WBNB, BTCB; BTCB — USDC.
fn peers(t: Address) -> Vec<Address> {
    if t == R_WBNB {
        vec![R_USDT, R_CAKE, R_USDC]
    } else if t == R_USDC {
        vec![R_USDT, R_WBNB, R_BTCB]
    } else if t == R_BTCB {
        vec![R_USDC]
    } else {
        vec![]
    }
}

#[test]
fn reach_by_depth() {
    let mids = [R_WBNB, R_USDC, R_BTCB];
    assert_eq!(reach(&peers, &[R_WBNB], &mids, 0), vec![R_WBNB]);
    assert_eq!(reach(&peers, &[R_WBNB], &mids, 1), vec![R_WBNB, R_USDC]);
    assert_eq!(
        reach(&peers, &[R_WBNB], &mids, 2),
        vec![R_WBNB, R_USDC, R_BTCB]
    );
    assert_eq!(
        reach(&peers, &[R_WBNB], &mids, 10),
        vec![R_WBNB, R_USDC, R_BTCB]
    );
    // Пул «промежуточная — промежуточная» (WBNB–USDC): обе стартовые, без повторов.
    assert_eq!(
        reach(&peers, &[R_WBNB, R_USDC], &mids, 1),
        vec![R_WBNB, R_USDC, R_BTCB]
    );
}

/// Ядро на базе во временной папке (как tests/core_dry_mode.rs).
async fn start(name: &str) -> (Core, Store) {
    let d = std::env::temp_dir().join(format!("arb-midwake-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    (core, settings)
}

async fn set(s: &Store, name: &str, v: Option<Value>) {
    let sc = Scope::Venue("bsc_dex".into());
    s.set(name, sc, v, "тест").await.unwrap();
}

fn coin(ticker: &str, a: Address) -> CoinEntry {
    CoinEntry {
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: Some(a),
        },
        presence: vec![],
        block: None,
        problem: None,
        wait: None,
        cex_cex: None,
        cex_cex_off: vec![],
        cex_cex_deny: vec![],
    }
}

/// Поколение памяти котировок монеты (растёт при пробуждении).
fn gen_of(core: &Core, a: Address) -> u64 {
    lk(&core.inner.quote_cache).generation(&VenueId("bsc_dex".into()), a)
}

#[tokio::test]
async fn wake_on_recorded_pools() {
    use route_common::{BTCB, CAKE, USDT, WBNB};
    let (core, s) = start("recorded").await;
    let dex = VenueId("bsc_dex".into());
    *wr(&core.inner.usdt) = Ok(USDT);
    // Пулы записи — в состояние рынка ядра; монета ONLY — пул записи WBNB-USDT с её адресом вместо WBNB.
    let p = route_common::parts().await;
    for e in p.registry.all() {
        let _ = wr(&core.inner.market).apply(MarketEvent::Pool(e.pool.clone()));
    }
    let got = route_common::load("tests/data/routes_2026-10-29");
    let only = route_common::v2(
        &got,
        "WBNB-USDT v2",
        address!("0x00000000000000000000000000000000000000aa"),
        ONLY,
        USDT,
    );
    let mut only_pool = only.pool.clone();
    only_pool.token0 = ONLY;
    only_pool.token1 = USDT;
    let _ = wr(&core.inner.market).apply(MarketEvent::Pool(only_pool));
    arb::core::ticker_index::rebuild(&core, &[coin("CAKE", CAKE), coin("ONLY", ONLY)]).await;
    let mark = PoolMark {
        at: Utc::now(),
        block: 1,
    };
    set(&s, "dex.intermediate_enabled", Some(json!(true))).await;
    set(&s, "dex.intermediate_tokens", Some(json!([WBNB, BTCB]))).await;
    set(&s, "dex.router_mode", Some(json!("graph"))).await;
    set(&s, "dex.route_max_hops", Some(json!(3))).await;

    // Пул промежуточной пары WBNB-USDT: CAKE (CAKE → WBNB → USDT) будится, ONLY — нет.
    let (c0, o0) = (gen_of(&core, CAKE), gen_of(&core, ONLY));
    mid_wake::wake(&core, &dex, (WBNB, USDT), mark);
    assert_eq!(
        gen_of(&core, CAKE),
        c0 + 1,
        "CAKE разбужена, память сброшена"
    );
    assert_eq!(
        gen_of(&core, ONLY),
        o0,
        "монета без связи с WBNB не будится"
    );

    // Пул «промежуточная — промежуточная» BTCB-WBNB (CAKE → BTCB → WBNB → USDT).
    mid_wake::wake(&core, &dex, (BTCB, WBNB), mark);
    assert_eq!(gen_of(&core, CAKE), c0 + 2);
    assert_eq!(gen_of(&core, ONLY), o0);

    // Пул без промежуточной (CAKE-USDT) — mid_wake никого не будит (монету пула будит tick.rs по токенам).
    mid_wake::wake(&core, &dex, (CAKE, USDT), mark);
    assert_eq!(gen_of(&core, CAKE), c0 + 2);

    // Классика (0 шагов): событие пула промежуточной монеты CAKE будит её.
    set(&s, "dex.router_mode", None).await;
    mid_wake::wake(&core, &dex, (WBNB, USDT), mark);
    assert_eq!(gen_of(&core, CAKE), c0 + 3);

    // Промежуточные выключены — никого.
    set(&s, "dex.intermediate_enabled", Some(json!(false))).await;
    mid_wake::wake(&core, &dex, (WBNB, USDT), mark);
    assert_eq!(gen_of(&core, CAKE), c0 + 3);
    assert_eq!(gen_of(&core, ONLY), o0);
}
