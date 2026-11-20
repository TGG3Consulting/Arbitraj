//! Файл: tests/arb_dex_algebra_dir_family.rs
//! Что делает: решение 444 (T433-F5) — подключение вида algebra_dir_fee (Algebra V1.9 с направленными комиссиями,
//! Camelot V3) без сети: таблица сопоставления (dex.kind → Proto::AlgebraDirFee, вид ядра Cl, в графе, не tradable —
//! исполнение шагом ALGEBRA исполнителя), обязательные настройки (фабрика, хеши кода пула — ревью 444; квотер —
//! виртуальный: с dex.quoter_virtual = да dex.quoter не нужен), события подписки (Swap/Mint/Burn как у V1.9 + Fee(uint16, uint16)).
//! Расчёт и события на записи узла — tests/arb_dex_algebra_dir.rs, tests/arb_dex_algebra_dir_events.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (families, exec_kind, events, abi_algebra_dir, virtual_quoter), arb::settings
//! (настоящая SQLite во временной папке).

use alloy::sol_types::SolEvent;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::types::PoolKind;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::abi_algebra::IAlgebraPool;
use arb::venues::bsc_dex::pools::abi_algebra_dir::IAlgebraDirPool;
use arb::venues::bsc_dex::pools::events;
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::quote::virtual_quoter;

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-algdir-{tag}-{}", std::process::id()));
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
        place: arb::venues::bsc_dex::ctx::Place::BSC,
        chains: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

#[test]
fn kind_maps_to_directional_algebra() {
    let f = families::make("camelot-v3-t444", "algebra_dir_fee").unwrap();
    assert_eq!(f.proto, Proto::AlgebraDirFee);
    assert_eq!(f.kind, PoolKind::Cl);
    assert_eq!((f.graphable, f.tradable, f.fork), (true, false, false));
    assert_eq!(f.executor_kind, Some(ExecKind::Algebra));
    assert!(virtual_quoter::supports(Proto::AlgebraDirFee));
    let t = events::topics(f);
    for h in [
        IAlgebraPool::Swap::SIGNATURE_HASH,
        IAlgebraPool::Mint::SIGNATURE_HASH,
        IAlgebraPool::Burn::SIGNATURE_HASH,
        IAlgebraDirPool::Fee::SIGNATURE_HASH,
    ] {
        assert!(t.contains(&h), "{h}");
    }
    assert_eq!(
        events::liquidity_topics(f),
        vec![IAlgebraPool::Mint::SIGNATURE_HASH]
    );
}

#[tokio::test]
async fn requires_factory_and_virtual_quoter() {
    let c = ctx("req").await;
    let id = "camelot-v3-t444";
    let f = families::make(id, "algebra_dir_fee").unwrap();
    assert_eq!(
        families::required(&c, f),
        vec!["dex.factory", "dex.pool_code_hashes", "dex.quoter"]
    );
    c.settings
        .set("dex.quoter_virtual", Scope::Dex(id.into()), Some(json!(true)), "тест")
        .await
        .unwrap();
    assert_eq!(families::required(&c, f), vec!["dex.factory", "dex.pool_code_hashes"]);
}
