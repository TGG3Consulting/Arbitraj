//! Файл: tests/bsc_dex_multi_pair_golden_dir.rs
//! Что делает: эталонный снимок справочника и отбора ПАРНЫХ пулов (решение 295, шаг 0) на живом ответе GeckoTerminal
//! 2026-10-24 (tests/data/indexers_2026-10-24/arb_indexers_2026-10-24.json, пять монет: CAKE, ETH, XRP, TWT, DOGE):
//! pools/index.rs::take (кандидаты, итог проверки, статусы по строке пула) и directory::snapshot (строки справочника
//! для интерфейса). Узла в тесте нет — пулы подключённой DEX «не проверен» (сбой узла — не отказ). Снимок записан кодом
//! ДО правки многомонетных пулов; после каждого шага правки — тот же текст байт в байт (tests/golden_common).
//! Адрес монеты — самый частый токен среди пар её страницы (из записи); встречные — USDT и WBNB (из записи).
//! Кто вызывает: cargo test --test bsc_dex_multi_pair_golden_dir.
//! От чего зависит: arb::venues::bsc_dex::{pools::index, directory}, arb::settings (настоящая SQLite во временной папке).

mod golden_common;

use std::collections::{HashMap, HashSet};

use alloy_primitives::{Address, address};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::directory::{self, TokenEntry, gt_parse};
use arb::venues::bsc_dex::pools::index;

use golden_common::{check, sorted};

const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");
const WBNB: Address = address!("0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c");
/// PancakeSwap V3 Factory (BNB) — официальная страница адресов (как tests/bsc_dex_index.rs).
const PCS_V3_FACTORY: Address = address!("0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");

/// Окружение на настоящей SQLite без настроек. Принимает: метку папки. Ошибок нет (паника теста).
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-mp-golden-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    {
        let c = Ctx {
            raw: RawLog::start(d.join("raw"), settings.clone()),
            pace: Default::default(),
            failover: Default::default(),
            evm_chain: Default::default(),
            place: arb::venues::bsc_dex::ctx::Place::BSC,
            chains: Default::default(),
            account: None,
            index: Default::default(),
            settings,
            vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
            http: reqwest::Client::new(),
            journal,
        };
        // Решение 416: тикер газа площадки DEX — настройка (dex.gas_ticker).
        c.settings
            .set(
                "dex.gas_ticker",
                arb::settings::scope::Scope::Venue("bsc_dex".into()),
                Some(serde_json::json!("BNB")),
                "тест",
            )
            .await
            .unwrap();
        c
    }
}

/// Задаёт настройку. Ошибок нет (паника теста).
async fn set(c: &Ctx, n: &str, s: Scope, v: Value) {
    c.settings.set(n, s, Some(v), "тест").await.unwrap();
}

/// Настройки записи: PancakeSwap V3 подключена (адреса, кроме фабрики, условные — сеть не нужна), Uniswap — не
/// поддерживается, размер части проверки 50. Ошибок нет.
pub async fn configure(c: &Ctx) {
    let a = json!("0x1111111111111111111111111111111111111111");
    let s = Scope::Dex("pancakeswap-v3-bsc".into());
    for (n, v) in [
        ("dex.kind", json!("pancakeswap_v3")),
        ("dex.pool_enabled", json!(true)),
        ("dex.universal_router", a.clone()),
        (
            "dex.universal_router_version",
            json!("pancakeswap_infinity"),
        ),
        ("dex.permit2", a.clone()),
        ("dex.factory", json!(format!("{PCS_V3_FACTORY}"))),
        ("dex.tick_lens", a.clone()),
        ("dex.quoter", a),
    ] {
        set(c, n, s.clone(), v).await;
    }
    set(
        c,
        "dex.kind",
        Scope::Dex("uniswap-bsc".into()),
        json!("unsupported"),
    )
    .await;
    set(
        c,
        "dex.verify_batch",
        Scope::Venue("bsc_dex".into()),
        json!(50),
    )
    .await;
}

/// Справочник пяти монет из записи: адрес монеты — самый частый токен её страницы. Ошибок нет (паника теста).
fn fill(c: &Ctx) {
    let body =
        std::fs::read_to_string("tests/data/indexers_2026-10-24/arb_indexers_2026-10-24.json")
            .unwrap();
    let all: Value = serde_json::from_str(&body).unwrap();
    for t in ["CAKE", "ETH", "XRP", "TWT", "DOGE"] {
        let pools =
            gt_parse::pools(all[format!("gt_pools_{t}_p1")]["body"].as_str().unwrap()).unwrap();
        let mut n: HashMap<Address, usize> = HashMap::new();
        for p in &pools {
            *n.entry(p.base).or_default() += 1;
            *n.entry(p.quote).or_default() += 1;
        }
        let coin = *n.iter().max_by_key(|(a, k)| (**k, **a)).unwrap().0;
        c.index.with(|s| {
            s.quotes = HashSet::from([USDT, WBNB]);
            s.tokens.insert(
                coin,
                TokenEntry {
                    ticker: t.into(),
                    pools,
                    ..Default::default()
                },
            );
        });
    }
}

/// Снимок: итог take, статусы по строке пула, строки справочника. Ошибок нет (паника теста).
pub async fn snapshot(c: &Ctx) -> String {
    let took = index::take(c, None).await.unwrap();
    let mut text = format!(
        "## take found={} failed={}\n",
        took.found.len(),
        took.failed
    );
    text += "## status\n";
    text += &sorted(
        c.index
            .with(|s| s.status.iter().map(|(k, v)| format!("{k} {v:?}")).collect()),
    );
    text += "## view\n";
    let v = directory::snapshot(c);
    text += &sorted(
        v.pools
            .iter()
            .map(|p| serde_json::to_string(p).unwrap())
            .collect(),
    );
    text
}

#[tokio::test]
async fn paired_directory_unchanged() {
    let c = ctx("dir").await;
    configure(&c).await;
    fill(&c);
    check("directory_2026-10-24", &snapshot(&c).await);
}

/// Решение 295, (д) с настройкой: dex.multi_pair = да у парной DEX (настройку не читает) и у подключённой многомонетной
/// DEX (Ellipsis, пулов в записи нет) — режим включён (Snap.multi_dex не пуст), а итог по парным пулам — тот же снимок.
#[tokio::test]
async fn paired_directory_unchanged_with_multi_pair() {
    let c = ctx("dir-mp").await;
    configure(&c).await;
    let e = Scope::Dex("ellipsis".into());
    set(&c, "dex.kind", e.clone(), json!("curve_stableswap")).await;
    set(&c, "dex.pool_enabled", e.clone(), json!(true)).await;
    set(&c, "dex.stable_pools", e.clone(), json!([])).await;
    set(&c, "dex.multi_pair", e, json!(true)).await;
    set(
        &c,
        "dex.multi_pair",
        Scope::Dex("pancakeswap-v3-bsc".into()),
        json!(true),
    )
    .await;
    fill(&c);
    let text = snapshot(&c).await;
    assert_eq!(
        c.index.with(|s| s.multi_dex.clone()),
        HashSet::from(["ellipsis".to_string()])
    );
    check("directory_2026-10-24", &text);
}
