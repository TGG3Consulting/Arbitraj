//! Файл: tests/api_coins_row.rs
//! Что делает: t84 — строка одной монеты GET /api/coins/{address} (api/coins.rs `one`) совпадает со строкой этой монеты
//! в полном списке GET /api/coins (`list`) — для КАЖДОЙ монеты с адресом. Справочник ядра собран тем же путём, что
//! пересборка (core/registry_build.rs build_part: рабочие и неподвижные монеты), из записанных справочников сетей бирж
//! Gate (cex_samples_2026_11_04/gate_spot_currencies.json), KuCoin (kucoin_currencies_v3.json) и BingX
//! (bingx_2026-10-24/config_getall.json). Настоящие ядро, сервер состояния (AppState) и SQLite во временной папке;
//! площадок в реестре нет (depth — null в обоих ответах). Плюс: неизвестный адрес — 404, неразобранный — 400.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::api::{state, coins}, arb::core (registry_build, coin_book), разбор сетей gate / kucoin / bingx.
//! Моков нет.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use serde_json::Value;
use tokio::sync::{Notify, broadcast};

use arb::api::coins;
use arb::api::state::AppState;
use arb::core::Core;
use arb::core::coin_book::CoinBook;
use arb::core::deps::Deps;
use arb::core::market_state::MarketState;
use arb::core::registry::Inputs;
use arb::core::registry_build::{Part, build_part};
use arb::core::types::{CoinNetworkInfo, Pool, VenueId};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;

const DIR: &str = "tests/data/cex_samples_2026_11_04";

fn json(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("файл записи")).expect("JSON")
}

fn v(s: &str) -> VenueId {
    VenueId(s.into())
}

/// Сети трёх бирж из записанных ответов (как tests/core_coin_by_address.rs).
fn networks() -> HashMap<VenueId, Vec<CoinNetworkInfo>> {
    let gate = arb::venues::gate::networks_parse::parse_networks(
        &json(&format!("{DIR}/gate_spot_currencies.json")),
        &Value::Array(vec![]),
        "BSC",
        &v("gate"),
        false,
        &HashMap::new(),
        &|_| Some(2),
    )
    .unwrap()
    .list;
    let mut k = json(&format!("{DIR}/kucoin_currencies_v3.json"));
    let kd = k.as_object_mut().unwrap().remove("data").unwrap();
    let kucoin = arb::venues::kucoin::networks_parse::parse(&kd, "bsc", &v("kucoin"), false)
        .unwrap()
        .list;
    let b = json("tests/data/bingx_2026-10-24/config_getall.json");
    let bingx =
        arb::venues::bingx::wallet::parse_networks(&b["data"], "BEP20", &v("bingx")).unwrap();
    HashMap::from([
        (v("gate"), gate),
        (v("kucoin"), kucoin),
        (v("bingx"), bingx),
    ])
}

/// Справочник ядра из записанных сетей: рабочие и неподвижные монеты (как core/rebuild.rs).
fn book() -> CoinBook {
    let n = networks();
    let kinds = vec![
        (v("bingx"), VenueKind::Cex),
        (v("gate"), VenueKind::Cex),
        (v("kucoin"), VenueKind::Cex),
    ];
    let networks = n
        .iter()
        .map(|(k, l)| (k.clone(), Arc::new(l.clone())))
        .collect();
    let quote = kinds
        .iter()
        .map(|(k, _)| (k.clone(), "USDT".to_string()))
        .collect();
    let bsc = HashMap::from([
        (v("bingx"), "BEP20".to_string()),
        (v("gate"), "BSC".to_string()),
        (v("kucoin"), "bsc".to_string()),
    ]);
    let fresh = kinds.iter().map(|(k, _)| (k.clone(), Ok(()))).collect();
    let market = MarketState::default();
    let pool_ok = |_: &Pool| Ok(());
    let inter = HashMap::new();
    let inp = Inputs {
        venues: &kinds,
        networks: &networks,
        usdt: None,
        quote: &quote,
        bsc_network: &bsc,
        pairs_fresh: &fresh,
        market: &market,
        pool_ok: &pool_ok,
        intermediate: &inter,
        native: None,
    };
    CoinBook {
        live: Arc::new(build_part(&inp, Part::Live)),
        inert: Arc::new(build_part(&inp, Part::Inert)),
        inert_key: None,
    }
}

/// Сервер состояния без площадок в реестре, ядро на SQLite во временной папке.
async fn state() -> AppState {
    let d = std::env::temp_dir().join(format!("arb-api-coins-row-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx.clone());
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let http = reqwest::Client::new();
    let registry = Registry::new();
    let notifier = Notifier {
        http: http.clone(),
        settings: settings.clone(),
        vault: vault.clone(),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: registry.clone(),
        settings: settings.clone(),
        journal: journal.clone(),
        notifier: notifier.clone(),
        db: db.clone(),
        changes: changes.clone(),
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    AppState {
        db,
        settings,
        journal,
        registry,
        vault,
        notifier,
        http,
        changes,
        journal_live: jtx,
        core,
        secrets_wake: Arc::new(Notify::new()),
    }
}

#[tokio::test]
async fn row_equals_full_list_row() {
    let st = state().await;
    let b = book();
    assert!(
        !b.live.is_empty() && !b.inert.is_empty(),
        "в записях нет рабочих или неподвижных монет"
    );
    *st.core.inner.coins.write().unwrap() = b;
    let all = coins::list(State(st.clone())).await.0;
    let list = all.as_array().expect("массив");
    let mut checked = 0;
    for full in list {
        let Some(addr) = full["asset"]["bsc_contract"].as_str() else {
            continue;
        };
        let row = coins::one(State(st.clone()), Path(addr.to_string()))
            .await
            .ok()
            .expect("строка монеты")
            .0;
        assert_eq!(
            &row, full,
            "строка {addr} не совпадает со строкой полного списка"
        );
        // Адрес в нижнем регистре (как ключ строки в интерфейсе) — та же строка.
        let low = coins::one(State(st.clone()), Path(addr.to_lowercase()))
            .await
            .ok()
            .unwrap()
            .0;
        assert_eq!(&low, full);
        checked += 1;
    }
    eprintln!(
        "монет в списке {}, сверено строк по адресу {checked}",
        list.len()
    );
    assert!(
        checked > 0,
        "в записях нет монет с адресом — тест ничего не проверил"
    );
    // Неизвестный адрес — 404; не адрес — 400.
    let none = "0x0000000000000000000000000000000000000001".to_string();
    let e = coins::one(State(st.clone()), Path(none))
        .await
        .err()
        .unwrap();
    assert_eq!(e.0, axum::http::StatusCode::NOT_FOUND, "{}", e.1);
    let e = coins::one(State(st.clone()), Path("names-x".into()))
        .await
        .err()
        .unwrap();
    assert_eq!(e.0, axum::http::StatusCode::BAD_REQUEST, "{}", e.1);
}
