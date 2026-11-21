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
//!     событием пула её промежуточной. Ревью t160: пул «монета — промежуточная» (ONLY-WBNB, CAKE-WBNB) — никого (Б1);
//!     глубина — DEEP (пул только к BTCB) будится событием WBNB-USDT в graph с 3 хопами и не будится в классике;
//!     повтор пары в том же блоке не будит (Б2); signal.mid_pool_wake не задана — ничего.
//! Сверка с перечнем путей адаптера на записанном графе — tests/bsc_dex_route_paths.rs (mid_wake_covers_every_path);
//! слияние событий одной монеты в один пересчёт — план tick_plan.rs (tests/core_tick_plan_322.rs).
//! Справочник и USDT ставятся перед каждым пробуждением: изменение dex.intermediate_* пересобирает справочник ядра
//! (без площадок — пустой), см. install.
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
use arb::core::types::{Asset, VenueId};
use arb::gateway::events::MarketEvent;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
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
            chain: None,
            ticker: ticker.into(),
            contract: Some(a),
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

/// Справочник монет и адрес USDT ядра — прямо перед пробуждением. Изменение настроек dex.intermediate_* толкает
/// пересборку справочника ядра (rebuild.rs, REGISTRY): у ядра теста площадок нет, пересборка ставит пустой справочник и
/// USDT «не загружен» (rebuild.rs:171-173) — поэтому ставим их заново после каждого изменения настроек и до wake (тест
/// идёт в однопоточном рантайме: между этой расстановкой и синхронным wake задача пересборки не выполняется). Проверка
/// справочника — чтобы пустой справочник давал ясную ошибку, а не «монета не разбужена». Ошибок нет.
async fn install(core: &Core, usdt: Address, coins: &[CoinEntry]) {
    *wr(&core.inner.usdt) = Ok(usdt);
    arb::core::ticker_index::rebuild(core, coins).await;
    for c in coins {
        let a = c.asset.contract.unwrap();
        assert!(
            !arb::core::engine::rd(&core.inner.tickers)
                .by_tokens((a, a))
                .is_empty(),
            "монеты {} нет в справочнике ядра перед пробуждением",
            c.asset.ticker
        );
    }
}

/// Пул записи (состояние WBNB-USDT V2 записи) с подставленными токенами — пул монеты, которой в записи нет.
fn pool_of(
    got: &std::collections::HashMap<String, Vec<u8>>,
    addr: Address,
    t0: Address,
    t1: Address,
) -> arb::core::types::Pool {
    let mut p = route_common::v2(got, "WBNB-USDT v2", addr, t0, t1).pool;
    p.token0 = t0;
    p.token1 = t1;
    p
}

/// Событие пула (отметка потока тика) и проход рассылки — как tick.rs + задача mid_wake.rs. Возвращает: монет в проходе.
fn event(core: &Core, dex: &VenueId, pool: (Address, Address), block: u64) -> usize {
    mid_wake::note(core, dex, pool, block);
    mid_wake::pass(core)
}

#[tokio::test]
async fn wake_on_recorded_pools() {
    use route_common::{BTCB, CAKE, USDT, WBNB};
    // DEEP — монета только с пулом к BTCB (маршрут DEEP → BTCB → WBNB → USDT, 3 хопа).
    const DEEP: Address = address!("0x00000000000000000000000000000000000000dd");
    let (core, s) = start("recorded").await;
    let dex = VenueId("bsc_dex".into());
    // Пулы записи — в состояние рынка ядра: CAKE-USDT ×2, CAKE-WBNB, WBNB-USDT, CAKE-BTCB, BTCB-WBNB.
    let p = route_common::parts().await;
    let dex2 = dex.clone();
    // Как tick.rs: пул в состояние рынка ядра, первый пул пары — в соседей токенов (mid_wake::peer).
    let apply = |pool: arb::core::types::Pool| {
        let t = (pool.token0, pool.token1);
        let _ = wr(&core.inner.market).apply(MarketEvent::Pool(pool));
        mid_wake::peer(&core, &dex2, t);
    };
    for e in p.registry.all() {
        apply(e.pool.clone());
    }
    let got = route_common::load("tests/data/routes_2026-10-29");
    let extra = [
        (
            address!("0x00000000000000000000000000000000000000aa"),
            ONLY,
            USDT,
        ),
        (
            address!("0x00000000000000000000000000000000000000bb"),
            DEEP,
            BTCB,
        ),
    ];
    for (a, t0, t1) in extra {
        apply(pool_of(&got, a, t0, t1));
    }
    let coins = [coin("CAKE", CAKE), coin("ONLY", ONLY), coin("DEEP", DEEP)];
    s.set(
        "signal.mid_pool_wake",
        arb::settings::scope::Scope::Global,
        Some(json!(true)),
        "тест",
    )
    .await
    .unwrap();
    set(&s, "dex.intermediate_enabled", Some(json!(true))).await;
    set(&s, "dex.intermediate_tokens", Some(json!([WBNB, BTCB]))).await;
    set(&s, "dex.router_mode", Some(json!("graph"))).await;
    set(&s, "dex.route_max_hops", Some(json!(3))).await;
    let g = |a: Address| gen_of(&core, a);
    let (c0, o0, d0) = (g(CAKE), g(ONLY), g(DEEP));

    // graph, 3 хопа: WBNB-USDT будит CAKE (CAKE → WBNB → USDT) и DEEP (DEEP → BTCB → WBNB → USDT); ONLY — нет.
    install(&core, USDT, &coins).await;
    event(&core, &dex, (WBNB, USDT), 10);
    assert_eq!(g(CAKE), c0 + 1, "CAKE разбужена, память сброшена");
    assert_eq!(
        g(DEEP),
        d0 + 1,
        "глубина: у DEEP нет пула с WBNB, маршрут через BTCB"
    );
    assert_eq!(g(ONLY), o0, "монета без связи с WBNB не будится");
    // Тот же пул в том же блоке — второй раз не будит.
    install(&core, USDT, &coins).await;
    event(&core, &dex, (WBNB, USDT), 10);
    assert_eq!(g(CAKE), c0 + 1, "повтор пары в том же блоке не будит");

    // Пул «промежуточная — промежуточная» BTCB-WBNB.
    install(&core, USDT, &coins).await;
    event(&core, &dex, (BTCB, WBNB), 11);
    assert_eq!((g(CAKE), g(DEEP), g(ONLY)), (c0 + 2, d0 + 2, o0));

    // Ревью Б1: пул «монета — промежуточная» (ONLY-WBNB, CAKE-WBNB) — только маршрут своей монеты: mid_wake никого.
    install(&core, USDT, &coins).await;
    assert_eq!(event(&core, &dex, (ONLY, WBNB), 12), 0);
    assert_eq!(event(&core, &dex, (CAKE, WBNB), 12), 0);
    // Пул без промежуточной (CAKE-USDT) — тоже никого.
    assert_eq!(event(&core, &dex, (CAKE, USDT), 12), 0);
    assert_eq!((g(CAKE), g(DEEP), g(ONLY)), (c0 + 2, d0 + 2, o0));

    // Классика (0 шагов): WBNB-USDT будит CAKE (пул к WBNB), DEEP (только к BTCB) — нет.
    set(&s, "dex.router_mode", None).await;
    install(&core, USDT, &coins).await;
    event(&core, &dex, (WBNB, USDT), 13);
    assert_eq!((g(CAKE), g(DEEP)), (c0 + 3, d0 + 2));

    // Промежуточные выключены — никого.
    set(&s, "dex.intermediate_enabled", Some(json!(false))).await;
    install(&core, USDT, &coins).await;
    assert_eq!(event(&core, &dex, (WBNB, USDT), 14), 0);
    assert_eq!((g(CAKE), g(DEEP), g(ONLY)), (c0 + 3, d0 + 2, o0));

    // Настройка выключена — отметки нет, проход пуст.
    s.set(
        "signal.mid_pool_wake",
        arb::settings::scope::Scope::Global,
        None,
        "тест",
    )
    .await
    .unwrap();
    set(&s, "dex.intermediate_enabled", Some(json!(true))).await;
    install(&core, USDT, &coins).await;
    assert_eq!(event(&core, &dex, (WBNB, USDT), 15), 0);
}
