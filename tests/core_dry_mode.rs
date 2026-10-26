//! Файл: tests/core_dry_mode.rs
//! Что делает: сухой режим (решение 74) на настоящем ядре и SQLite во временной папке: dry.enabled = да — вход
//! исполнителя (executor::accepted) отказывает первым же шагом, ничего не запускает и не пишет сделок (расчёт —
//! записанные CAKE: asks BingX, пул V2); тот же расчёт без сухого режима проходит этот замок и останавливается дальше
//! («направление не торгуемо» — площадок в реестре нет), то есть отказ в сухом режиме дал именно он; режим читается на
//! лету; dry.enabled не задана — обычный режим; включён без dry.budget_usdt — бюджета нет, причина названа; ссылки
//! карточки по шаблонам из настроек (пара — по площадке, пул — по DEX).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, tests/size_common. Моков нет: площадок в
//! реестре нет, проверяется решение ядра.

mod size_common;

use arb::core::Core;
use arb::core::deps::Deps;
use arb::core::dry::{self, Mode};
use arb::core::executor::{self, Start};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use serde_json::json;
use size_common::{Rec, d};
use tokio::sync::broadcast;

/// Ядро на базе во временной папке.
async fn start(name: &str) -> (Core, Store) {
    let d = std::env::temp_dir().join(format!("arb-dry-{name}-{}", std::process::id()));
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

/// Задаёт или очищает настройку всей системы.
async fn set(s: &Store, name: &str, v: Option<serde_json::Value>) {
    s.set(name, Scope::Global, v, "тест").await.unwrap();
}

#[tokio::test]
async fn dry_mode_never_calls_executor() {
    let (core, s) = start("gate").await;
    let rec = Rec::load("dry-gate").await;
    let calc = size_common::calc::calc(&rec, d("10"));
    // Не задано — обычный режим.
    assert_eq!(dry::mode(&s), Mode::Live);
    set(&s, "dry.enabled", Some(json!(true))).await;
    set(&s, "dry.budget_usdt", Some(json!("1000"))).await;
    assert_eq!(dry::mode(&s), Mode::Dry(Ok(d("1000"))));
    assert!(dry::blocks(&s).is_some());
    let r = executor::accepted(&core, calc.clone()).await;
    assert!(matches!(r, Start::Dry(_)), "{r:?}");
    assert!(core.deals().is_empty(), "сделки нет");
    // Включение снято на лету — замок пропускает, дальше отказ ядра (площадок в реестре нет).
    set(&s, "dry.enabled", Some(json!(false))).await;
    assert_eq!(dry::mode(&s), Mode::Live);
    assert_eq!(executor::accepted(&core, calc).await, Start::NotTradable);
    assert!(core.deals().is_empty());
}

#[tokio::test]
async fn dry_without_budget_has_reason() {
    let (_core, s) = start("budget").await;
    set(&s, "dry.enabled", Some(json!(true))).await;
    match dry::mode(&s) {
        Mode::Dry(Err(e)) => assert!(e.contains("dry.budget_usdt"), "{e}"),
        m => panic!("ожидался сухой режим без бюджета: {m:?}"),
    }
    // Без бюджета ничего не исполняется.
    assert!(dry::blocks(&s).is_some());
}

/// Ссылки карточки по настройкам (решение 74): не задан шаблон — причина; пара — по площадке, пул — по DEX.
#[tokio::test]
async fn link_templates_from_settings() {
    use arb::core::card_links::{pair, pool};
    use arb::core::types::PoolRef;
    let (_core, s) = start("links").await;
    let p = PoolRef::Contract(size_common::PAIR);
    let l = pair(&s, "bingx", "CAKE");
    assert!(l.url.is_none() && l.why.unwrap().contains("venue.pair_url_template"));
    let bx = Scope::Venue("bingx".into());
    s.set("venue.quote_asset", bx.clone(), Some(json!("USDT")), "тест")
        .await
        .unwrap();
    s.set(
        "venue.pair_url_template",
        bx,
        Some(json!("https://bingx.com/en/spot/{symbol}")),
        "тест",
    )
    .await
    .unwrap();
    assert_eq!(
        pair(&s, "bingx", "CAKE").url.as_deref(),
        Some("https://bingx.com/en/spot/CAKEUSDT")
    );
    let dex = "pancakeswap-v2-bsc";
    assert!(pool(&s, "bsc_dex", dex, &p).why.unwrap().contains(dex));
    s.set(
        "dex.pool_url_template",
        Scope::Dex(dex.into()),
        Some(json!("https://www.geckoterminal.com/bsc/pools/{pool}")),
        "тест",
    )
    .await
    .unwrap();
    assert_eq!(
        pool(&s, "bsc_dex", dex, &p).url.as_deref(),
        Some("https://www.geckoterminal.com/bsc/pools/0xa39af17ce4a8eb807e076805da1e2b8ea7d0755b")
    );
    // Другая DEX — свой шаблон, этот не подставляется.
    assert!(pool(&s, "bsc_dex", "uniswap-v4-bsc", &p).url.is_none());
}
