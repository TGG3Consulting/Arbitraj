//! Файл: tests/core_cold_coins.rs
//! Что делает: решение 131 — справочник монет ядра делится на рабочие и неподвижные монеты (core/coin_book.rs).
//! Проверяется на живых ответах BingX 2026-10-24 (tests/data/bingx_2026-10-24: config/getall, symbols) и записанном
//! стакане CAKE (tests/data/bingx/ws_spot_2026-10-23.jsonl), на работающем ядре, без перезапуска:
//! - сборка частями даёт тот же справочник, что целиком (registry_build::build_part);
//! - монета без сети BSC (справочник сетей «до» — тот же ответ без строк BEP20 у CAKE) — на экране «Монеты» с
//!   причиной, но не среди рабочих монет; повторная пересборка с тем же справочником неподвижные монеты не собирает;
//! - у монеты появилась сеть BSC (новый справочник сетей — весь записанный ответ, как его приносит registry_sync.rs) —
//!   следующая пересборка переводит её в рабочие монеты на лету: данные сети BSC, стакан, нет причин отсева — монета
//!   уходит в подписку биржи и в направления, как раньше; среди неподвижных её больше нет.
//!
//! Направлений у одной биржи нет (нужна вторая площадка) — их строит тот же CoinEntry::directions по рабочим монетам.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (engine, rebuild, coin_book, registry_build, market_state), arb::venues::bingx, SQLite.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::DateTime;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::market_state::MarketState;
use arb::core::registry::{self, Exclusion, Inputs};
use arb::core::registry_build::{Part, build_part};
use arb::core::types::{CoinNetworkInfo, Pool, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::events::MarketEvent;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bingx::depth::{self, Msg};
use arb::venues::bingx::pairs::Snapshot;
use arb::venues::bingx::{symbols, wallet};

/// Время ответа symbols в сыром журнале (мс) — tests/data/bingx_2026-10-24/README.md.
const SYMBOLS_AT_MS: i64 = 1792868538956;

fn bingx() -> VenueId {
    VenueId("bingx".into())
}

fn read(name: &str) -> Value {
    let raw = std::fs::read_to_string(format!("tests/data/bingx_2026-10-24/{name}")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// Сети монет из записанного config/getall (сеть BSC у BingX — BEP20).
fn nets() -> Vec<CoinNetworkInfo> {
    wallet::parse_networks(&read("config_getall.json")["data"], "BEP20", &bingx()).unwrap()
}

/// Справочник сетей «до»: тот же ответ без сети BSC у CAKE.
fn nets_without_cake_bsc() -> Vec<CoinNetworkInfo> {
    nets()
        .into_iter()
        .filter(|i| !(i.asset.ticker == "CAKE" && i.network == "BEP20"))
        .collect()
}

/// SpotPairs и стакан CAKE — как их шлёт адаптер BingX.
fn market_events(n: &[CoinNetworkInfo]) -> Vec<MarketEvent> {
    let s = symbols::Symbols::default();
    let at = DateTime::from_timestamp_millis(SYMBOLS_AT_MS).unwrap();
    s.load(
        symbols::parse_list(&read("symbols.json")["data"], "USDT").unwrap(),
        at,
    );
    let assets: Vec<_> = n
        .iter()
        .filter(|i| i.asset.bsc_contract.is_some())
        .map(|i| i.asset.clone())
        .collect();
    let mut out = vec![Snapshot::build(&s, &assets).event(bingx(), at)];
    let raw = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    for line in raw.lines() {
        let m: Value = serde_json::from_str(line).unwrap();
        if let Msg::Depth {
            symbol,
            bids,
            asks,
            exchange_time,
        } = depth::parse(m["m"].as_str().unwrap())
            && symbol == "CAKE-USDT"
        {
            let a = n.iter().find(|i| i.asset.ticker == "CAKE").unwrap();
            out.push(MarketEvent::Book(depth::to_book(
                a.asset.clone(),
                bids,
                asks,
                exchange_time,
            )));
            return out;
        }
    }
    panic!("в записи нет стакана CAKE-USDT")
}

/// Ядро на базе во временной папке, площадка BingX включена (реестр шлюза пуст — подписка пишет сбой в журнал).
async fn start() -> Core {
    let d = std::env::temp_dir().join(format!("arb-core-cold-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let v = Scope::Venue("bingx".into());
    for (n, val) in [
        ("venue.enabled", json!(true)),
        ("venue.quote_asset", json!("USDT")),
        ("venue.bsc_network", json!("BEP20")),
        // Записи 2026-10-24: справочник пар должен считаться свежим и сейчас.
        ("venue.symbols_stale_s", json!(1_000_000_000u64)),
    ] {
        settings.set(n, v.clone(), Some(val), "тест").await.unwrap();
    }
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![(bingx(), VenueKind::Cex)];
    core
}

#[test]
fn parts_together_equal_whole() {
    let n = nets();
    let mut m = MarketState::default();
    for ev in market_events(&n) {
        m.apply(ev);
    }
    let kinds = vec![(bingx(), VenueKind::Cex)];
    let networks = HashMap::from([(bingx(), Arc::new(n))]);
    let quote = HashMap::from([(bingx(), "USDT".to_string())]);
    let bsc = HashMap::from([(bingx(), "BEP20".to_string())]);
    let fresh = HashMap::from([(bingx(), Ok(()))]);
    let intermediate = HashMap::new();
    let pool_ok = |_: &Pool| Ok(());
    let inp = Inputs {
        venues: &kinds,
        networks: &networks,
        usdt: None,
        quote: &quote,
        bsc_network: &bsc,
        pairs_fresh: &fresh,
        market: &m,
        pool_ok: &pool_ok,
        intermediate: &intermediate,
    };
    let whole = registry::build(&inp);
    let live = build_part(&inp, Part::Live);
    let inert = build_part(&inp, Part::Inert);
    assert_eq!(live.len() + inert.len(), whole.len());
    for c in &whole {
        let found = live.iter().chain(&inert).find(|x| x.asset == c.asset);
        assert_eq!(found, Some(c), "{}", c.asset.ticker);
    }
    // Неподвижные — без адреса контракта (PONS: сети BEP20 нет; BNB, ZBC, RAMP: BEP20 без адреса).
    assert!(inert.iter().all(|c| c.asset.bsc_contract.is_none()));
    for t in ["PONS", "BNB", "ZBC", "RAMP"] {
        assert!(inert.iter().any(|c| c.asset.ticker == t), "{t}");
    }
    assert!(live.iter().any(|c| c.asset.ticker == "CAKE"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coin_gets_bsc_network_and_goes_hot_without_restart() {
    let core = start().await;
    let full = nets();
    for ev in market_events(&full) {
        core.inner.market.write().unwrap().apply(ev);
    }
    // «До»: у CAKE нет сети BSC.
    core.inner
        .networks
        .write()
        .unwrap()
        .insert(bingx(), Arc::new(nets_without_cake_bsc()));
    arb::core::rebuild::rebuild(&core).await;
    let before = core.coins();
    assert!(
        !before.iter().any(|c| c.asset.ticker == "CAKE"),
        "CAKE среди рабочих"
    );
    if let Some(c) = before.all().find(|c| c.asset.ticker == "CAKE") {
        // У CAKE в ответе есть и другие сети — на экране «Монеты» она видна с причиной, как раньше.
        assert_eq!(c.presence[0].excluded, vec![Exclusion::NoBscNetwork]);
    }
    // Та же пересборка с тем же справочником — неподвижные монеты не собираются заново (тот же снимок).
    arb::core::rebuild::rebuild(&core).await;
    let same = core.coins();
    assert!(Arc::ptr_eq(&before.inert, &same.inert));
    // «После»: новый справочник сетей (весь записанный ответ) — без перезапуска CAKE переходит в рабочие.
    core.inner
        .networks
        .write()
        .unwrap()
        .insert(bingx(), Arc::new(full));
    arb::core::rebuild::rebuild(&core).await;
    let after = core.coins();
    let cake = after
        .iter()
        .find(|c| c.asset.ticker == "CAKE")
        .expect("CAKE среди рабочих монет");
    let p = cake.at(&bingx()).expect("CAKE на BingX без причин отсева");
    assert!(
        p.info
            .as_ref()
            .is_some_and(|i| i.asset.bsc_contract.is_some())
    );
    assert!(!after.inert.iter().any(|c| c.asset.ticker == "CAKE"));
    assert_eq!(after.total(), after.all().count());
}
