//! Файл: tests/api_wallets_exec_net_491.rs
//! Что делает: решение 491 — на экране «Маппинг кошельков» запрет развёртывания исполнителя (executor_deploy_refused)
//! в строке КАЖДОЙ сети кошелька берётся у площадки DEX ИМЕННО этой сети (её dex.evm_chain_id), а не у площадки DEX по
//! умолчанию. Регрессия: раньше view считал запрет один раз на площадке по умолчанию (bsc_dex) и клал его в каждый блок
//! сети — в блоке Arbitrum показывался статус BSC. На настоящем сервере состояния (AppState), ядре, адаптерах BingX,
//! MEXC и ДВУХ площадках DEX (bsc_dex + arb_dex тем же кодом адаптера, решение 435) и SQLite во временной папке.
//! Записи приёмки нет ни у одной версии для синтетических chainId сторон — отказ развёртывания детерминирован при любой
//! собранной версии исполнителя и переживёт запись приёмки v5 (её серв делает на живых пулах реального chainId 42161,
//! задача T488-02): проверяется ПО КАКОЙ СЕТИ считается отказ (в тексте — chainId сети площадки), не его наличие у
//! конкретной сборки.
//! Ключ — официальный тестовый вектор Hardhat Network, счёт #0 (адрес 0xf39F…2266).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::api::{state, wallets, wallets_view}, arb::core, arb::venues::bsc_dex, arb::settings, arb::secrets.
//! Моков нет: отказ считает настоящий deploy_gate по вшитой записи приёмки сборки contracts/out/.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde_json::json;
use tokio::sync::{Notify, broadcast};
use zeroize::Zeroizing;

use arb::api::state::AppState;
use arb::api::wallets::{self, AddBody};
use arb::api::wallets_view;
use arb::core::Core;
use arb::core::deps::Deps;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::BscDex;
use arb::venues::bsc_dex::ctx::{Ctx, Place};
use arb::venues::bsc_dex::exec::executor::code::{self, Version};
use arb::venues::bsc_dex::exec::executor::live_proof;

const KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
/// Синтетические chainId сторон: не настоящие сети — записи приёмки для них нет ни у одной версии, поэтому отказ есть при
/// любой собранной версии исполнителя и после появления записи v5 для реального 42161 тест не ломается.
const BSC_SYN: u64 = 10056;
const ARB_SYN: u64 = 1042161;
/// Две площадки DEX процесса (решение 435: id из dex.venues, один код адаптера).
const ALL: &[&str] = &["bsc_dex", "arb_dex"];

/// Состояние веб-сервера с BingX, MEXC и двумя площадками DEX (bsc_dex, arb_dex), каждой задан свой dex.evm_chain_id.
async fn state() -> AppState {
    let d = std::env::temp_dir().join(format!("arb-wallets-exec-net-491-{}", std::process::id()));
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
    // Решение 416: тикер газа площадки DEX — настройка; решение 461: chainId сети площадки — dex.evm_chain_id площадки;
    // решение 416/430: имя сети площадки — dex.chain (bsc / arbitrum), нужна, чтобы ядро выбрало площадку DEX
    // (dex_of/dex_chain); синтетические chainId сторон (dex.evm_chain_id) не трогаем — отказ деплоя считается по ним.
    for (venue, ticker, chain, net) in [
        ("bsc_dex", "BNB", BSC_SYN, "bsc"),
        ("arb_dex", "ETH", ARB_SYN, "arbitrum"),
    ] {
        settings
            .set(
                "dex.gas_ticker",
                Scope::Venue(venue.into()),
                Some(json!(ticker)),
                "тест",
            )
            .await
            .unwrap();
        settings
            .set(
                "dex.evm_chain_id",
                Scope::Venue(venue.into()),
                Some(json!(chain)),
                "тест",
            )
            .await
            .unwrap();
        settings
            .set(
                "dex.chain",
                Scope::Venue(venue.into()),
                Some(json!(net)),
                "тест",
            )
            .await
            .unwrap();
    }
    // Решение 430: две площадки DEX в разных сетях — сеть по умолчанию задаётся явно (иначе dex_chain откажет).
    settings
        .set(
            "dex.default_chain",
            Scope::Global,
            Some(json!("bsc")),
            "тест",
        )
        .await
        .unwrap();
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
    // Площадка DEX сети BSC (bsc_dex) — процесс знает обе (place.all = ALL, решение 435).
    let dx = Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw,
        pace: Default::default(),
        failover: Default::default(),
        evm_chain: Default::default(),
        place: Place {
            id: "bsc_dex",
            all: ALL,
        },
        chains: Default::default(),
        account: None,
        index: Default::default(),
    };
    // Решение 435: вторая площадка DEX (arb_dex) — тот же код адаптера, своя сеть (её dex.evm_chain_id).
    let arb_ctx = dx.for_place("arb_dex");
    registry
        .register(Arc::new(arb::venues::bingx::Bingx::new(bx)))
        .await;
    registry
        .register(Arc::new(arb::venues::mexc::Mexc::new(mx)))
        .await;
    registry.register(Arc::new(BscDex::new(dx))).await;
    registry.register(Arc::new(BscDex::new(arb_ctx))).await;
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

/// Тело добавления кошелька (пароль «pw», как в state).
fn body(name: &str, key: &str) -> Json<AddBody> {
    Json(AddBody {
        name: name.into(),
        key: Zeroizing::new(format!("0x{key}")),
        password: Zeroizing::new("pw".into()),
    })
}

/// Строка сети блока chains[] по id площадки. Паникует, если блока нет.
fn chain<'a>(chains: &'a serde_json::Value, venue: &str) -> &'a serde_json::Value {
    chains
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["venue"] == json!(venue))
        .unwrap_or_else(|| panic!("нет блока сети {venue}: {chains}"))
}

#[tokio::test]
async fn refusal_is_per_network_not_default_venue() {
    let st = state().await;
    // Кошелёк один на все сети (решение 432); отказ развёртывания считается по площадке, не зависит от разблокировки.
    let _ = wallets::add(State(st.clone()), body("BingX", KEY))
        .await
        .ok()
        .unwrap();
    let v = wallets_view::view(&st).await;
    let chains = &v["wallets"][0]["chains"];
    // Две площадки DEX — блок на каждую сеть (решение 432).
    assert_eq!(chains.as_array().unwrap().len(), 2, "блоки сетей: {chains}");

    let bsc = chain(chains, "bsc_dex")["executor_deploy_refused"]
        .as_str()
        .expect("отказ bsc_dex — строка (записи приёмки для синтетического chainId нет)");
    let arb = chain(chains, "arb_dex")["executor_deploy_refused"]
        .as_str()
        .expect("отказ arb_dex — строка (записи приёмки для синтетического chainId нет)");

    // Главное: отказ каждой строки считается по СВОЕЙ сети (её chainId в тексте), не по сети площадки по умолчанию.
    assert!(
        bsc.contains(&format!("chainId {BSC_SYN}")),
        "bsc_dex: ждали chainId {BSC_SYN}: {bsc}"
    );
    assert!(
        arb.contains(&format!("chainId {ARB_SYN}")),
        "arb_dex: ждали chainId {ARB_SYN}: {arb}"
    );
    // Блок Arbitrum не показывает статус сети BSC (та самая регрессия).
    assert!(
        !arb.contains(&format!("chainId {BSC_SYN}")),
        "arb_dex не должен содержать chainId сети BSC: {arb}"
    );
    assert_ne!(bsc, arb, "тексты отказа по сетям должны различаться");

    let current = code::current_version().unwrap();
    let check_classes = |block: &serde_json::Value, version, network, message: &str| {
        let rows = block["executor_v3_classes"].as_array().unwrap();
        let expected = live_proof::proofs(version, Some(network));
        assert!(!expected.is_empty());
        assert_eq!(rows.len(), expected.len());
        for (row, proof) in rows.iter().zip(expected) {
            assert_eq!(row["why"], json!(proof.why));
            assert!(row["live"].as_str().unwrap().contains(message));
        }
    };
    // У нового кошелька таблица обеих сетей соответствует готовому к развёртыванию коду.
    for (venue, network) in [("bsc_dex", BSC_SYN), ("arb_dex", ARB_SYN)] {
        check_classes(chain(chains, venue), current, network, "ещё не развёрнут");
    }

    // Адрес зарегистрирован только на BSC, его версия ещё не сверена: прежняя семантика v3;
    // Arbitrum по-прежнему показывает запись текущей сборки до развёртывания на этой сети.
    let id = v["wallets"][0]["id"].as_str().unwrap();
    let address = "0x0000000000000000000000000000000000000001";
    st.settings
        .accounts()
        .set_executor_at("bsc_dex", id, Some(address.into()), "тест")
        .await
        .unwrap();
    let v = wallets_view::view(&st).await;
    let chains = &v["wallets"][0]["chains"];
    check_classes(
        chain(chains, "bsc_dex"),
        Version::V3,
        BSC_SYN,
        "нет сверенного",
    );
    check_classes(
        chain(chains, "arb_dex"),
        current,
        ARB_SYN,
        "ещё не развёрнут",
    );
}
