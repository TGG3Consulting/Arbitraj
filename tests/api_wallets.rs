//! Файл: tests/api_wallets.rs
//! Что делает: API экрана «Маппинг кошельков» (решение 132) на настоящем сервере состояния (AppState), ядре, адаптерах
//! BingX, MEXC, DEX и SQLite во временной папке: ключ кошелька НИКОГДА не попадает в ответы API, историю и журнал
//! (только имя секрета и адрес, выведенный адаптером); ключ того же адреса, что у другого кошелька, — отказ; строки
//! маппинга — биржи реестра, «DEX ↔ DEX», «пробы»; неизвестная строка — отказ; кошелёк в маппинге не удаляется;
//! предупреждение «деньги смешиваются».
//! Ключ — официальный тестовый вектор Hardhat Network, счёт #0 (адрес 0xf39F…2266).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::api::{state, wallets, wallets_view}, arb::core, arb::venues, arb::settings, arb::secrets.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use serde_json::json;
use tokio::sync::{Notify, broadcast};
use zeroize::Zeroizing;

use arb::api::state::AppState;
use arb::api::wallets::{self, AddBody, HistoryQuery, MapBody};
use arb::core::Core;
use arb::core::deps::Deps;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;

const KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const ADDR: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

/// Состояние веб-сервера с тремя площадками и созданным файлом секретов (пароль «pw»).
async fn state() -> AppState {
    let d = std::env::temp_dir().join(format!("arb-api-wallets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx.clone());
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    for (n, v) in [
        ("security.kdf_memory_kib", 64),
        ("security.kdf_iterations", 1),
        ("security.kdf_parallelism", 1),
    ] {
        settings
            .set(n, Scope::Global, Some(json!(v)), "тест")
            .await
            .unwrap();
    }
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let p = arb::secrets::crypto::KdfParams {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };
    vault.create(Zeroizing::new("pw".into()), p).await.unwrap();
    let raw = RawLog::start(d.join("raw"), settings.clone());
    let http = reqwest::Client::new();
    let registry = Registry::new();
    let cex = |taps| {
        (
            settings.clone(),
            vault.clone(),
            http.clone(),
            journal.clone(),
            raw.clone(),
            taps,
        )
    };
    let (s, v, h, j, r, t) = cex(Default::default());
    let bx = arb::venues::bingx::ctx::Ctx {
        settings: s,
        vault: v,
        http: h,
        journal: j,
        raw: r,
        taps: t,
    };
    let (s, v, h, j, r, t) = cex(Default::default());
    let mx = arb::venues::mexc::ctx::Ctx {
        settings: s,
        vault: v,
        http: h,
        journal: j,
        raw: r,
        taps: t,
    };
    let dx = arb::venues::bsc_dex::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw,
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
    };
    registry
        .register(Arc::new(arb::venues::bingx::Bingx::new(bx)))
        .await;
    registry
        .register(Arc::new(arb::venues::mexc::Mexc::new(mx)))
        .await;
    registry
        .register(Arc::new(arb::venues::bsc_dex::BscDex::new(dx)))
        .await;
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

/// Тело добавления.
fn body(name: &str, key: &str) -> Json<AddBody> {
    Json(AddBody {
        name: name.into(),
        key: Zeroizing::new(format!("0x{key}")),
        password: Zeroizing::new("pw".into()),
    })
}

#[tokio::test]
async fn key_never_leaves_and_mapping_rules_hold() {
    let st = state().await;
    let low = KEY.to_lowercase();
    // Добавление: ответ без ключа, с именем секрета.
    let w = wallets::add(State(st.clone()), body("MEXC", KEY))
        .await
        .ok()
        .unwrap()
        .0;
    let text = w.to_string().to_lowercase();
    assert!(!text.contains(&low), "ключ в ответе: {text}");
    let id = w["id"].as_str().unwrap().to_string();
    assert_eq!(w["secret"], format!("wallet.{id}.key"));
    // Тот же ключ вторым кошельком — отказ (деньги смешались бы молча), запись не остаётся.
    let e = wallets::add(State(st.clone()), body("Ещё", KEY))
        .await
        .err()
        .unwrap();
    assert!(e.1.contains("того же адреса"), "{}", e.1);
    assert_eq!(st.settings.accounts().wallets().len(), 1);
    // Список: адрес — от адаптера, ключа нет.
    let v = wallets::list(State(st.clone())).await.0;
    let text = v.to_string();
    assert!(!text.to_lowercase().contains(&low), "ключ в списке");
    assert_eq!(
        v["wallets"][0]["address"].as_str().unwrap().to_lowercase(),
        ADDR.to_lowercase()
    );
    // Строки: биржи реестра (без перечисления в коде), «DEX ↔ DEX», «пробы».
    let rows: Vec<&str> = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["row"].as_str().unwrap())
        .collect();
    assert_eq!(rows, vec!["bingx", "mexc", "dex_dex", "probe"]);
    // Неизвестная строка — отказ; привязка к двум денежным строкам — предупреждение.
    let bad = MapBody {
        row: "gate".into(),
        wallet: Some(id.clone()),
    };
    assert!(wallets::map(State(st.clone()), Json(bad)).await.is_err());
    for row in ["mexc", "dex_dex"] {
        let m = MapBody {
            row: row.into(),
            wallet: Some(id.clone()),
        };
        let r = wallets::map(State(st.clone()), Json(m)).await.ok().unwrap();
        assert_eq!(r.0["changed"], true);
    }
    let v = wallets::list(State(st.clone())).await.0;
    assert_eq!(v["shared"][0]["rows"], json!(["dex_dex", "mexc"]));
    // В маппинге — не удаляется.
    let e = wallets::remove(State(st.clone()), Path(id.clone()))
        .await
        .err()
        .unwrap();
    assert!(e.1.contains("маппинге"), "{}", e.1);
    // История и журнал — без ключа.
    let h = wallets::history(State(st.clone()), Query(HistoryQuery { limit: 100 }))
        .await
        .ok()
        .unwrap()
        .0;
    assert!(!json!(h).to_string().to_lowercase().contains(&low));
    let j = st.journal.list(1000, None).await.unwrap();
    assert!(
        !json!(j).to_string().to_lowercase().contains(&low),
        "ключ в журнале"
    );
}
