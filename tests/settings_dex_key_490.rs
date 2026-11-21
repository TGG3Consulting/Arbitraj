//! Файл: tests/settings_dex_key_490.rs
//! Что делает: решение 490 — правило суффикса сети ключа области «DEX» (settings/dex_key.rs suffix / venue_suffix) и
//! проверка нового ключа «id@сеть» из «Добавить DEX» (check_net) на настоящей SQLite (Store::load), без моков:
//! (а) две площадки DEX (bsc_dex сети bsc по умолчанию, arb_dex сети arbitrum): суффикс arb_dex — «@arbitrum», у
//! bsc_dex — нет; одна площадка — суффикса нет без чтения настроек; не задана dex.default_chain при двух — ошибка;
//! (б) «x@arbitrum» принимается при площадке arb_dex сети arbitrum; без такой площадки — отказ «нет площадки DEX сети»;
//! «x@bsc» — отказ «сеть по умолчанию»; «x» без «@» — как раньше (не проверяется);
//! (в) уже существующий ключ (есть значение) не проверяется — живые настройки не запираются.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::settings::{dex_key, scope, store}, arb::storage. Моков нет.

use serde_json::json;
use tokio::sync::broadcast;

use arb::settings::dex_key::{check_net, venue_suffix};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;

/// Новое хранилище настроек в своей папке. Паника — SQLite не открылась.
async fn fresh(name: &str) -> Store {
    let dir = std::env::temp_dir().join(format!("arb-dex-key-490-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (tx, _) = broadcast::channel(16);
    let (jtx, _) = broadcast::channel(16);
    Store::load(db.clone(), tx, Journal::new(db, jtx))
        .await
        .unwrap()
}

/// Две площадки DEX как в живом боте двух сетей: bsc_dex — bsc (сеть по умолчанию), arb_dex — arbitrum.
/// Паника — запись не прошла.
async fn two_nets(name: &str) -> Store {
    let s = fresh(name).await;
    for (n, sc, v) in [
        ("dex.chain", Scope::Venue("bsc_dex".into()), "bsc"),
        ("dex.chain", Scope::Venue("arb_dex".into()), "arbitrum"),
        ("dex.default_chain", Scope::Global, "bsc"),
    ] {
        s.set(n, sc, Some(json!(v)), "тест").await.unwrap();
    }
    s
}

/// Id площадок. Ошибок нет.
fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|x| x.to_string()).collect()
}

#[tokio::test]
async fn suffix_rule_is_venue_rule() {
    let s = two_nets("suffix").await;
    assert_eq!(
        venue_suffix(&s, "arb_dex", false).unwrap().as_deref(),
        Some("@arbitrum")
    );
    assert_eq!(venue_suffix(&s, "bsc_dex", false).unwrap(), None);
    // Одна площадка DEX — её сеть по умолчанию: настройки не читаются (пустое хранилище — тоже None).
    let empty = fresh("suffix-alone").await;
    assert_eq!(venue_suffix(&empty, "bsc_dex", true).unwrap(), None);
    // Две площадки без dex.default_chain — суффикс не определить.
    let half = fresh("suffix-half").await;
    half.set(
        "dex.chain",
        Scope::Venue("arb_dex".into()),
        Some(json!("arbitrum")),
        "тест",
    )
    .await
    .unwrap();
    let e = venue_suffix(&half, "arb_dex", false).unwrap_err();
    assert!(e.message.contains("dex.default_chain"), "{}", e.message);
}

#[tokio::test]
async fn network_key_needs_dex_venue_of_that_net() {
    let s = two_nets("check").await;
    let both = ids(&["arb_dex", "bsc_dex"]);
    assert!(check_net(&s, "dodo-amm-dvm@arbitrum", &both).is_ok());
    // Площадки arb_dex в шлюзе нет — сеть arbitrum некому читать.
    let e = check_net(&s, "dodo-amm-dvm@arbitrum", &ids(&["bsc_dex"])).unwrap_err();
    assert!(e.contains("нет площадки DEX сети «arbitrum»"), "{e}");
    // Опечатка в имени сети — мёртвый ключ: отказ.
    let e = check_net(&s, "dodo-amm-dvm@arbitrun", &both).unwrap_err();
    assert!(e.contains("нет площадки DEX сети «arbitrun»"), "{e}");
    // Сеть по умолчанию — ключ без «@».
    let e = check_net(&s, "lista-stable@bsc", &both).unwrap_err();
    assert!(e.contains("сеть по умолчанию"), "{e}");
    // Без «@» — как раньше, не проверяется.
    assert!(check_net(&s, "lista-stable", &both).is_ok());
    assert!(check_net(&s, "lista-stable", &ids(&["bsc_dex"])).is_ok());
}

#[tokio::test]
async fn existing_key_not_locked() {
    let s = fresh("known").await;
    // Ключ уже с значением (записан до проверки, например импортом) — не проверяется.
    s.set(
        "dex.kind",
        Scope::Dex("camelot@arbitrum".into()),
        Some(json!("uniswap_v2")),
        "тест",
    )
    .await
    .unwrap();
    assert!(check_net(&s, "camelot@arbitrum", &ids(&["bsc_dex"])).is_ok());
    // Новый ключ той же сети в том же хранилище — проверяется: площадки arbitrum нет.
    assert!(check_net(&s, "zyberswap-v3@arbitrum", &ids(&["bsc_dex"])).is_err());
}
