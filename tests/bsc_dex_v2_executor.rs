//! Файл: tests/bsc_dex_v2_executor.rs
//! Что делает: решение 280 — V2-форк без роутера (dex.kind = v2_fork_executor) без сети и без моков:
//! (1) вид: spec / exec_kind::of / make — в графе, без своего пути, форк, шаг V2; required — без dex.router (у v2_fork —
//!   с ним, как раньше); classify подключает DEX с фабрикой и комиссией без роутера;
//! Разбор РЕАЛЬНЫХ симуляций сверки (покупка у шести DEX, продажа у двух; вызовы — quote/v2_pair_calls.rs) —
//! tests/bsc_dex_v2_executor_sell.rs (запись tests/data/v2_executor_balance_2026-11-15; прежняя раскладка вызовов
//! записи v2_fork_executor_2026-11-15 после ревью не используется сверкой). Исполнимость графа и откат покупки пробы —
//! tests/bsc_dex_v2_executor_graph.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex (families, exec_kind, quote::v2_pair_quote),
//! arb::settings (настоящая SQLite во временной папке).

use serde_json::json;
use tokio::sync::broadcast;

use arb::core::types::PoolKind;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::exec_kind::{self, ExecKind};
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::quote::v2_pair_quote;

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-v2x-{tag}-{}", std::process::id()));
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
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

/// Вид v2_fork_executor и неизменный v2_fork: флаги, вид шага, обязательные настройки, подключение без роутера.
#[tokio::test]
async fn kind_flags_and_required() {
    let c = ctx("req").await;
    assert_eq!(
        families::spec("v2_fork_executor"),
        Some((PoolKind::V2, Proto::V2, true, false, true))
    );
    assert_eq!(
        families::spec("v2_fork"),
        Some((PoolKind::V2, Proto::V2, true, true, true))
    );
    assert_eq!(exec_kind::of("v2_fork_executor"), Some(ExecKind::V2));
    assert_eq!(exec_kind::of("v2_fork"), Some(ExecKind::V2));
    let fx = families::make("dao_swap", "v2_fork_executor").unwrap();
    let fk = families::make("biswap", "v2_fork").unwrap();
    assert!(v2_pair_quote::kind_on(fx) && !v2_pair_quote::kind_on(fk));
    for id in ["dao_swap", "biswap"] {
        let s = Scope::Dex(id.into());
        c.settings
            .set("dex.v2_fee_source", s, Some(json!("fixed")), "тест")
            .await
            .unwrap();
    }
    assert_eq!(
        families::required(&c, fx),
        vec!["dex.factory", "dex.v2_fee_pct"]
    );
    assert_eq!(
        families::required(&c, fk),
        vec!["dex.factory", "dex.router", "dex.v2_fee_pct"]
    );
    assert!(
        !exec_kind::via_executor(&c, fx),
        "без исполнителя у кошелька — сигнал"
    );
    let s = Scope::Dex("dao_swap".into());
    for (n, v) in [
        ("dex.kind", json!("v2_fork_executor")),
        ("dex.pool_enabled", json!(true)),
        (
            "dex.factory",
            json!("0x940BEb635cbEeC04720AC97FADb97205676e6aa4"),
        ),
        ("dex.v2_fee_pct", json!("0.25")),
    ] {
        c.settings.set(n, s.clone(), Some(v), "тест").await.unwrap();
    }
    let f = families::classify(&c, "dao_swap").expect("подключается без dex.router");
    assert_eq!((f.graphable, f.tradable, f.fork), (true, false, true));
}
