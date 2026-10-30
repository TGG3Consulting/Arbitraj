//! Файл: tests/bsc_dex_index.rs
//! Что делает: пулы из справочника индексатора → пулы подписки (решение 58) без сети: таблица сопоставления
//! «DEX индексатора → вид» (не подключена, не поддерживается, выключена, не хватает адреса), итог по пулам CAKE из
//! живого ответа GeckoTerminal 2026-10-24, сбой узла — «не проверен», своя комиссия пары Biswap (swapFee / 1000),
//! очистка устаревших настроек.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools, directory}, arb::settings (настоящая SQLite во временной папке).
//! Решения проверки пула и «кто меняет» — на живых ответах узла: tests/bsc_dex_node_verify.rs.

use std::collections::HashSet;

use alloy_primitives::{Address, U256, address};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::DirectoryStatus;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::directory::{TokenEntry, gt_parse};
use arb::venues::bsc_dex::pools::families::{self, Proto};
use arb::venues::bsc_dex::pools::index;
use arb::venues::bsc_dex::pools::verify;

const CAKE: Address = address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");
const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");
/// PancakeSwap V3 Factory (BNB) — официальная страница адресов (indexer_research.md, 2.1).
const PCS_V3_FACTORY: Address = address!("0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");

/// Окружение на настоящей SQLite без настроек.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-dex-index-{tag}-{}", std::process::id()));
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
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

/// Задаёт настройки. Принимает: [(имя, область, значение)].
async fn set(c: &Ctx, list: &[(&str, Scope, Value)]) {
    for (n, s, v) in list {
        c.settings
            .set(n, s.clone(), Some(v.clone()), "тест")
            .await
            .unwrap();
    }
}

/// Все настройки DEX вида PancakeSwap V3, кроме `skip` (адреса фабрики — официальный, остальные — условные: сеть не нужна).
fn v3_row(id: &str, skip: &str) -> Vec<(&'static str, Scope, Value)> {
    let a = json!("0x1111111111111111111111111111111111111111");
    let s = Scope::Dex(id.into());
    let all = vec![
        ("dex.kind", s.clone(), json!("pancakeswap_v3")),
        ("dex.pool_enabled", s.clone(), json!(true)),
        ("dex.universal_router", s.clone(), a.clone()),
        (
            "dex.universal_router_version",
            s.clone(),
            json!("pancakeswap_infinity"),
        ),
        ("dex.permit2", s.clone(), a.clone()),
        ("dex.factory", s.clone(), json!(format!("{PCS_V3_FACTORY}"))),
        ("dex.tick_lens", s.clone(), a.clone()),
        ("dex.quoter", s.clone(), a),
    ];
    all.into_iter().filter(|(n, _, _)| *n != skip).collect()
}

#[tokio::test]
async fn mapping_table_reasons() {
    let c = ctx("map").await;
    let e = families::classify(&c, "pancakeswap-v3-bsc").unwrap_err();
    assert_eq!(
        e,
        "DEX pancakeswap-v3-bsc не подключена: нет в таблице сопоставления (dex.kind)"
    );
    set(
        &c,
        &[(
            "dex.kind",
            Scope::Dex("thena-v3".into()),
            json!("unsupported"),
        )],
    )
    .await;
    assert!(
        families::classify(&c, "thena-v3")
            .unwrap_err()
            .contains("не поддерживается")
    );
    // V2-форк (Biswap, решения 59, 82): с 80.9a — в графе для поиска/сигналов (graphable), но tradable=false (авто-
    // сделкой не исполняется, нет Universal Router своей фабрики). Форку нужны фабрика/роутер/комиссия, НЕ UR/Permit2.
    let bis = Scope::Dex("biswap".into());
    let a = json!("0x1111111111111111111111111111111111111111");
    set(&c, &[("dex.kind", bis.clone(), json!("v2_fork"))]).await;
    // Только вид — ещё не подключён (нужны остальные настройки), но причина уже не «нет пути исполнения».
    assert!(
        families::classify(&c, "biswap")
            .unwrap_err()
            .contains("не задана dex.pool_enabled"),
        "{}",
        families::classify(&c, "biswap").unwrap_err()
    );
    set(
        &c,
        &[
            ("dex.pool_enabled", bis.clone(), json!(true)),
            ("dex.factory", bis.clone(), a.clone()),
            ("dex.router", bis.clone(), a.clone()),
            ("dex.v2_fee_source", bis.clone(), json!("fixed")),
            ("dex.v2_fee_pct", bis.clone(), json!("0.25")),
        ],
    )
    .await;
    let f = families::classify(&c, "biswap").unwrap();
    assert_eq!(
        (f.graphable, f.tradable),
        (true, false),
        "V2-форк: в графе (graphable), но не исполняется (tradable=false)"
    );
    set(&c, &v3_row("pancakeswap-v3-bsc", "dex.quoter")).await;
    let e = families::classify(&c, "pancakeswap-v3-bsc").unwrap_err();
    assert!(e.ends_with("не задана dex.quoter"), "{e}");
    set(&c, &v3_row("pancakeswap-v3-bsc", "")).await;
    let f = families::classify(&c, "pancakeswap-v3-bsc").unwrap();
    assert_eq!(
        (f.id, f.proto, f.tradable),
        ("pancakeswap-v3-bsc", Proto::PancakeV3, true)
    );
    assert!(families::still(&c, f).is_ok());
    // Вид изменили на лету — пул старого вида больше не котируется.
    set(
        &c,
        &[(
            "dex.kind",
            Scope::Dex("pancakeswap-v3-bsc".into()),
            json!("uniswap_v3"),
        )],
    )
    .await;
    assert!(families::still(&c, f).is_err());
    set(
        &c,
        &[(
            "dex.kind",
            Scope::Dex("pancakeswap-v3-bsc".into()),
            json!("pancakeswap_v3"),
        )],
    )
    .await;
    // Умный роутер: тот же Universal Router, тот же Permit2 и формат PancakeSwap Infinity.
    assert!(!families::smart_ok(&c, f), "smart.* не заданы");
    let v = Scope::Venue("bsc_dex".into());
    let a = json!("0x1111111111111111111111111111111111111111");
    set(
        &c,
        &[
            ("smart.universal_router", v.clone(), a.clone()),
            ("smart.permit2", v.clone(), a),
        ],
    )
    .await;
    assert!(families::smart_ok(&c, f));
    set(
        &c,
        &[(
            "smart.permit2",
            v,
            json!("0x2222222222222222222222222222222222222222"),
        )],
    )
    .await;
    assert!(
        !families::smart_ok(&c, f),
        "другой Permit2 — не умный маршрут"
    );
    set(
        &c,
        &[(
            "dex.pool_enabled",
            Scope::Dex("pancakeswap-v3-bsc".into()),
            json!(false),
        )],
    )
    .await;
    assert!(
        families::classify(&c, "pancakeswap-v3-bsc")
            .unwrap_err()
            .contains("выключена")
    );
    // Списка DEX в коде нет: строки — из настроек (unsupported в видах нет, форк есть — для настроек в интерфейсе).
    let ids: Vec<&str> = families::all(&c).iter().map(|f| f.id).collect();
    assert_eq!(ids, vec!["biswap", "pancakeswap-v3-bsc"]);
}

#[tokio::test]
async fn cake_directory_statuses() {
    let c = ctx("cake").await;
    let body =
        std::fs::read_to_string("tests/data/indexers_2026-10-24/arb_indexers_2026-10-24.json")
            .unwrap();
    let all: Value = serde_json::from_str(&body).unwrap();
    let pools = gt_parse::pools(all["gt_pools_CAKE_p1"]["body"].as_str().unwrap()).unwrap();
    c.index.with(|s| {
        s.quotes = HashSet::from([USDT]);
        s.tokens.insert(
            CAKE,
            TokenEntry {
                ticker: "CAKE".into(),
                pools,
                ..Default::default()
            },
        );
    });
    set(
        &c,
        &[(
            "dex.kind",
            Scope::Dex("uniswap-bsc".into()),
            json!("unsupported"),
        )],
    )
    .await;
    set(&c, &v3_row("pancakeswap-v3-bsc", "")).await;
    // Размер части проверки не задан — проверки нет вовсе (не задано — не работает).
    assert!(index::take(&c, None).await.is_err());
    set(
        &c,
        &[(
            "dex.verify_batch",
            Scope::Venue("bsc_dex".into()),
            json!(50),
        )],
    )
    .await;
    // Узла в тесте нет: оба пула PancakeSwap V3 — «не проверен» (сбой узла — не отказ, будет повтор).
    let took = index::take(&c, None).await.unwrap();
    assert!(took.found.is_empty());
    assert_eq!(took.failed, 2);
    let st = |pool: &str| c.index.with(|s| s.status.get(pool).cloned().unwrap());
    assert_eq!(
        st("0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1"),
        DirectoryStatus::Unchecked
    );
    assert!(
        matches!(st("0xfe4fe5b4575c036ac6d5cccfe13660020270e27a"), DirectoryStatus::NotConnected(w) if w.contains("не поддерживается"))
    );
    assert_eq!(
        st("0xa39af17ce4a8eb807e076805da1e2b8ea7d0755b"),
        DirectoryStatus::NotConnected(
            "DEX pancakeswap_v2 не подключена: нет в таблице сопоставления (dex.kind)".into()
        )
    );
    let v = arb::venues::bsc_dex::directory::snapshot(&c);
    assert_eq!(v.pools.len(), 8, "6 пулов CAKE/USDT и 2 — с нативным BNB");
}

#[tokio::test]
async fn orphan_setting_only_cleared() {
    let c = ctx("orphan").await;
    let s = Scope::Dex("pancakeswap".into());
    // Значение настройки, которой больше нет в каталоге, задать нельзя.
    assert!(
        c.settings
            .set("dex.enabled", s.clone(), Some(json!(true)), "тест")
            .await
            .is_err()
    );
    // Очистить несуществующее значение тоже нельзя — нечего удалять.
    assert!(
        c.settings
            .set("dex.enabled", s, None, "тест")
            .await
            .is_err()
    );
}

#[test]
fn biswap_pair_fee() {
    // BiswapPair.sol: swapFee = 1 по умолчанию (0,1%), максимум 1000; формула balance·1000 − amountIn·swapFee.
    assert_eq!(verify::v2_fee_pips(U256::from(1u8), 1000).unwrap(), 1000);
    assert_eq!(verify::v2_fee_pips(U256::from(2u8), 1000).unwrap(), 2000);
    assert!(verify::v2_fee_pips(U256::from(1000u32), 1000).is_err());
    assert!(
        verify::v2_fee_pips(U256::from(1u8), 0)
            .unwrap_err()
            .contains("= 0")
    );
}

#[tokio::test]
async fn orphan_setting_stored_is_cleared() {
    let d = std::env::temp_dir().join(format!("arb-dex-index-orphan2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    // Значение, оставшееся от прежней версии (dex.enabled удалена из каталога, решение 60).
    db.call(|c| {
        c.execute(
            "INSERT INTO settings (name, scope, value, updated_at) VALUES ('dex.enabled', 'dex:pancakeswap', 'true', 'x')",
            [],
        )
    })
    .await
    .unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let store = Store::load(db, changes, journal).await.unwrap();
    let s = Scope::Dex("pancakeswap".into());
    assert!(store.get_exact("dex.enabled", &s).is_some());
    let change = store
        .set("dex.enabled", s.clone(), None, "тест")
        .await
        .unwrap();
    assert!(change.is_some_and(|c| c.new.is_none()));
    assert!(store.get_exact("dex.enabled", &s).is_none());
}
