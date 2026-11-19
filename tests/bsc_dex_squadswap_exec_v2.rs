//! Файл: tests/bsc_dex_squadswap_exec_v2.rs
//! Что проверяет: задача t114 — обратный вызов squadV3SwapCallback (SquadSwap V3, 0xc4e21d86, решение 281) в
//! ArbExecutor v2 и проверка dex.v3_swap_callback по версии исполнителя, СВЕРЕННОЙ у кошелька (pools/exec_callback.rs).
//! Данные — записанные: пулы SquadSwap V3 из tests/data/squadswap_v3_2026-11-15/probe.json (блок 133618816), код
//! исполнителя — реальные артефакты contracts/out/ArbExecutor.bin-runtime (v2) и contracts/out/v1/ (v1). Сценарии:
//! - исполнителя у кошелька нет — проверки обратного вызова нет (решает executor_set), пути через исполнитель нет;
//! - исполнитель есть, версия не сверена — пути исполнения нет, причина «не сверен»;
//! - v1 — пути нет (в коде v1 селектора нет), причина «нужен исполнитель новой версии»;
//! - v2 — путь Squad открыт (via_executor, describe, шаг V3 по адресу записанного пула);
//! - снова v1 (память по ключу (версия, сигнатура), а не по одной сигнатуре) — снова закрыт.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{ctx, exec::executor::entry, pools::{exec_callback, exec_kind, families}},
//! arb::settings (настоящая SQLite во временной папке).

use alloy_primitives::Address;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::PoolRef;
use arb::settings::accounts::ROW_PROBE;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::executor::entry;
use arb::venues::bsc_dex::pools::exec_kind::ExecKind;
use arb::venues::bsc_dex::pools::{exec_callback, exec_kind, families};

const DEX: &str = "squadswap-v3-bsc";
const SQUAD: &str = "squadV3SwapCallback(int256,int256,bytes)";
/// Адрес исполнителя кошелька теста (код по нему тест не читает: версию пишет сам, как onchain.rs после сверки).
const EXECUTOR: &str = "0x00000000000000000000000000000000000000ee";

fn probe() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/squadswap_v3_2026-11-15/probe.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Окружение на настоящей SQLite (account = None — кошелёк строки «Пробы и сигналы»).
async fn ctx() -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-squad-exec-v2-{}", std::process::id()));
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
        chains: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

/// Кошелёк строки «Пробы и сигналы» с исполнителем EXECUTOR (версия не сверена). Возвращает: id кошелька.
async fn wallet_with_executor(c: &Ctx) -> String {
    let a = c.settings.accounts();
    let id = a.reserve_id().await.unwrap();
    a.add(&id, "тест", "wallet.test.key", "bsc_dex", "тест")
        .await
        .unwrap();
    a.set_executor(&id, Some(EXECUTOR.into()), "тест")
        .await
        .unwrap();
    a.map_set(ROW_PROBE, Some(&id), "тест").await.unwrap();
    id
}

/// Версия, записанная у кошелька, как после сверки кода (onchain.rs → set_executor_version).
async fn verified(c: &Ctx, id: &str, v: &str) {
    let wrapped = (v == "v2").then(|| format!("{:#x}", Address::repeat_byte(0xbb)));
    c.settings
        .accounts()
        .set_executor_version(id, EXECUTOR, Some(v.into()), wrapped, "тест")
        .await
        .unwrap();
}

#[tokio::test]
async fn squad_exec_path_follows_verified_executor_version() {
    let c = ctx().await;
    let f = families::make(DEX, "v3_fork_pancake").unwrap();
    assert_eq!(f.executor_kind, Some(ExecKind::V3));
    let s = Scope::Dex(DEX.into());
    c.settings
        .set(exec_callback::SETTING, s, Some(json!(SQUAD)), "тест")
        .await
        .unwrap();
    // Исполнителя нет — проверки обратного вызова нет, пути через исполнитель нет (executor_set).
    assert_eq!(exec_callback::missing(&c, f), None);
    assert!(!exec_kind::via_executor(&c, f));
    // Исполнитель есть, версия не сверена — пути нет, причина «не сверен».
    let id = wallet_with_executor(&c).await;
    let why = exec_callback::missing(&c, f).expect("не сверен");
    assert!(why.contains("не сверен") && why.contains(EXECUTOR), "{why}");
    assert!(!exec_kind::via_executor(&c, f));
    // v1 — селектора 0xc4e21d86 в contracts/out/v1/ArbExecutor.bin-runtime нет: путь закрыт.
    verified(&c, &id, "v1").await;
    let why = exec_callback::missing(&c, f).expect("v1 — вызова нет");
    assert!(
        why.contains("v1") && why.contains("0xc4e21d86") && why.contains("новой версии"),
        "{why}"
    );
    assert!(!exec_kind::via_executor(&c, f));
    assert!(exec_kind::describe(&c, f).contains("без пути исполнения"));
    // v2 — вызов есть (t114): путь исполнения Squad открыт; шаг V3 — по адресу записанного пула.
    verified(&c, &id, "v2").await;
    assert_eq!(exec_callback::missing(&c, f), None);
    assert!(exec_kind::via_executor(&c, f));
    assert!(exec_kind::describe(&c, f).contains("исполнимо через ArbExecutor"));
    for p in probe()["pools"].as_array().unwrap() {
        let pool: Address = p["pool"].as_str().unwrap().parse().unwrap();
        assert_eq!(
            entry::supported(f, &PoolRef::Contract(pool)),
            Ok((ExecKind::V3, pool))
        );
    }
    // Снова v1 — память по (версия, сигнатура): итог v2 не переносится на v1.
    verified(&c, &id, "v1").await;
    assert!(exec_callback::missing(&c, f).is_some());
    assert!(!exec_kind::via_executor(&c, f));
}
