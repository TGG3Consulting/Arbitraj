//! Файл: tests/core_chain_430.rs
//! Что делает: решение 430 (шаг 2 двух сетей) — сети ядра на записанных данных живого бота и настоящей SQLite:
//! (а) одна площадка DEX — площадка DEX монеты без сети (запись живого бота) без чтения настроек, как раньше;
//! (б) две площадки DEX в разных сетях: сеть по умолчанию — dex.default_chain; не задана или не сеть площадки — ошибка;
//! (в) канонический вид: сеть по умолчанию — None (запись BSC прежняя), другая сеть — Some;
//! (г) площадка DEX сети: по сети монеты; у одной сети две площадки DEX — ошибка.
//! Записи: tests/data/sigma_windows_2026-11-16/deals_sell_on_cex.jsonl (поле direction — как в учёте бота).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{chain, types}, arb::settings, arb::storage. Моков нет.

use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::chain;
use arb::core::types::{ChainId, Direction, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;

const DEALS: &str = "tests/data/sigma_windows_2026-11-16/deals_sell_on_cex.jsonl";

/// Первое направление сделки из выгрузки живого бота. Паника — файла нет.
fn recorded() -> Direction {
    let raw = std::fs::read_to_string(DEALS).unwrap();
    let line = raw.lines().find(|l| !l.trim().is_empty()).unwrap();
    serde_json::from_value(serde_json::from_str::<Value>(line).unwrap()["direction"].clone())
        .unwrap()
}

/// Новое хранилище настроек в своей папке. Паника — SQLite не открылась.
async fn fresh(name: &str) -> Store {
    let dir = std::env::temp_dir().join(format!("arb-chain-430-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    Store::load(db.clone(), tx, Journal::new(db, jtx))
        .await
        .unwrap()
}

/// Задаёт настройку. Паника — запись не прошла.
async fn set(s: &Store, name: &str, scope: Scope, v: &str) {
    s.set(name, scope, Some(json!(v)), "тест").await.unwrap();
}

/// Площадка по id. Ошибок нет.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Сеть по id. Ошибок нет.
fn c(id: &str) -> ChainId {
    ChainId(id.into())
}

#[tokio::test]
async fn one_dex_answers_without_settings() {
    let s = fresh("one").await;
    let d = recorded();
    assert!(d.asset.chain.is_none(), "запись до решения 416");
    let one = [v("bsc_dex")];
    // dex.chain не задана — площадка DEX монеты без сети всё равно единственная (как «первая DEX» раньше).
    assert_eq!(
        chain::dex_of(&s, &one, d.asset.chain.as_ref()).unwrap(),
        v("bsc_dex")
    );
    set(&s, "dex.chain", Scope::Venue("bsc_dex".into()), "bsc").await;
    assert_eq!(
        chain::tag(&s, &one, &c("bsc")).unwrap(),
        None,
        "сеть по умолчанию — без сети"
    );
    assert_eq!(chain::dex_chain(&s, &one).unwrap(), c("bsc"));
}

#[tokio::test]
async fn two_chains_need_default_chain() {
    let s = fresh("two").await;
    let d = recorded();
    let two = [v("bsc_dex"), v("arb_dex")];
    set(&s, "dex.chain", Scope::Venue("bsc_dex".into()), "bsc").await;
    set(&s, "dex.chain", Scope::Venue("arb_dex".into()), "arbitrum").await;
    let e = chain::dex_of(&s, &two, d.asset.chain.as_ref()).unwrap_err();
    assert!(e.contains("dex.default_chain"), "{e}");
    set(&s, "dex.default_chain", Scope::Global, "solana").await;
    let e = chain::dex_chain(&s, &two).unwrap_err();
    assert!(e.contains("нет площадки DEX этой сети"), "{e}");
    set(&s, "dex.default_chain", Scope::Global, "bsc").await;
    assert_eq!(chain::dex_chain(&s, &two).unwrap(), c("bsc"));
    // Запись живого бота (без сети) — монета BSC, площадка bsc_dex; ключ записи не меняется.
    assert_eq!(chain::of(&s, &two, &d.asset).unwrap(), c("bsc"));
    assert_eq!(chain::dex_of(&s, &two, None).unwrap(), v("bsc_dex"));
    assert_eq!(chain::tag(&s, &two, &c("bsc")).unwrap(), None);
    assert_eq!(
        chain::tag(&s, &two, &c("arbitrum")).unwrap(),
        Some(c("arbitrum"))
    );
    assert_eq!(
        chain::dex_of(&s, &two, Some(&c("arbitrum"))).unwrap(),
        v("arb_dex")
    );
    let e = chain::dex_of(&s, &two, Some(&c("solana"))).unwrap_err();
    assert!(e.contains("нет площадки DEX сети solana"), "{e}");
}

#[tokio::test]
async fn one_chain_two_dex_is_error() {
    let s = fresh("same").await;
    let two = [v("bsc_dex"), v("bsc_dex2")];
    set(&s, "dex.chain", Scope::Venue("bsc_dex".into()), "bsc").await;
    set(&s, "dex.chain", Scope::Venue("bsc_dex2".into()), "bsc").await;
    let e = chain::dex_of(&s, &two, None).unwrap_err();
    assert!(e.contains("одна площадка DEX на сеть"), "{e}");
}

/// Сведения сетей — наблюдавшиеся публичные RPC 25.11.2026 UTC (URL, блок и хеш сохранены).
/// Одна неподготовленная сеть, повтор или неверная запись не отключают уже настроенные сети.
#[tokio::test]
async fn base_ethereum_registration_is_independent_and_opt_in() {
    use arb::settings::evm_profiles::PROFILES;
    use arb::venues::bsc_dex::{dex_venues, send::channels};
    let s = fresh("base-ethereum").await;
    let recorded: Value = serde_json::from_str(include_str!("data/base_ethereum_networks.json")).unwrap();
    assert_eq!(dex_venues::extra(&s), (vec![], vec![]));
    set(&s, "dex.default_chain", Scope::Global, "bsc").await;
    set(&s, "dex.chain", Scope::Venue("bsc_dex".into()), "bsc").await;
    set(&s, "dex.chain", Scope::Venue("arb_dex".into()), "arbitrum").await;
    s.set("dex.venues", Scope::Global, Some(json!(["arb_dex", "base_dex", "invalid-id", "eth_dex", "base_dex"])), "тест").await.unwrap();
    for p in PROFILES {
        let scope = Scope::Venue(p.venue.into());
        assert_eq!(p.evm_chain_id, recorded[p.chain]["chain"].as_u64().unwrap());
        assert!(s.set("dex.evm_chain_id", scope.clone(), Some(json!(56)), "тест").await.is_err());
        assert!(s.set("dex.gas_mode", scope.clone(), Some(json!("eip1559_l1")), "тест").await.is_err());
        assert!(s.set("exec.via_kyber", scope.clone(), Some(json!(true)), "тест").await.is_err());
        set(&s, "dex.chain", scope.clone(), p.chain).await;
        set(&s, "dex.gas_mode", scope.clone(), p.gas_mode).await;
        s.set("dex.evm_chain_id", scope, Some(json!(p.evm_chain_id)), "тест").await.unwrap();
        let (ids, reasons) = dex_venues::extra(&s);
        assert!(ids.iter().any(|id| id == p.venue));
        assert!(ids.contains(&"arb_dex".to_string()));
        assert!(reasons.iter().any(|r| r.contains("invalid-id")));
        if p.chain == "base" {
            assert!(reasons.iter().any(|r| r.contains("eth_dex")));
        }
        let available: Vec<_> = channels::ALL.iter().filter(|ch| channels::compatible(p.venue, ch)).map(|ch| ch.id).collect();
        assert_eq!(available, [p.public_channel]);
    }
    let (mut ids, reasons) = dex_venues::extra(&s);
    assert_eq!(ids, ["arb_dex", "base_dex", "eth_dex"]);
    assert_eq!(reasons.len(), 2);
    s.set("bsc.node_http_url", Scope::Global, Some(json!("https://bsc-rpc.publicnode.com")), "тест").await.unwrap();
    assert!(s.resolve("bsc.node_http_url", Some("base_dex"), None).is_none());
    assert!(s.resolve("bsc.node_http_url", Some("eth_dex"), None).is_none());
    assert!(s.resolve("bsc.node_http_url", Some("bsc_dex"), None).is_some());
    set(&s, "dex.chain", Scope::Venue("previous_dex".into()), "base").await;
    ids.push("previous_dex".into());
    assert_eq!(dex_venues::unique_profiles(&s, &mut ids).len(), 1);
    assert_eq!(ids, ["arb_dex", "eth_dex", "previous_dex"]);
    let venues = [v("bsc_dex"), v("incomplete"), v("eth_dex")];
    assert_eq!(chain::dex_of(&s, &venues, None).unwrap(), venues[0]);
    assert_eq!(chain::dex_of(&s, &venues, Some(&c("ethereum"))).unwrap(), venues[2]);
    assert!(chain::dex_of(&s, &venues, Some(&c("base"))).is_err());
}
