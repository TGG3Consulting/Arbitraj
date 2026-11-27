//! Проверки хранения кадров Motoswap для CI. Входы взяты из снимка фабрики и двух
//! защищённых модулей на блоке 26494988. Переключения лимита идут через штатный Store.
use super::*;
use crate::venues::bsc_dex::{hashflow::recovery_tests, pools::families};
use flate2::read::GzDecoder;
use serde_json::{Value, json};

/// Читает сохранённый JSON. Ошибка архива останавливает проверку.
fn archive(raw: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(raw)).unwrap()
}

/// Восстанавливает роли и проверенные тарифы из каталога фабрики на архивном блоке.
fn recorded(j: &Value) -> (Roles, Frame) {
    use super::super::super::Role;
    let d = &j["directory"];
    let address = |s: &str| d["roles"][s].as_str().unwrap().parse().unwrap();
    let hash = |s: &str| d["code"][s].as_str().unwrap().parse().unwrap();
    let role = |name: &str, implementation: Option<&str>| Role {
        address: address(name),
        hash: hash(name),
        implementation: implementation.map(|s| (address(s), hash(s))),
    };
    let roles = Roles {
        factory: role("factory", Some("factory_impl")),
        fee: role("fee_router", Some("fee_impl")),
        core: role("core", None),
        beacon: role("beacon", None),
        pair_impl: role("pair_impl", None),
        registry: role("registry", None),
        creators: role("creator", None),
        vault: role("vault", None),
        collector: role("collector", Some("collector_impl")),
        wrapped: address("wrapped"),
        init: d["init"].as_str().unwrap().parse().unwrap(),
    };
    let rate = |name: &str| u16::try_from(d[name].as_u64().unwrap()).unwrap();
    (roles, Frame {
        lp: rate("lp_bps"),
        protocol: rate("protocol_bps"),
        creator: rate("creator_bps"),
        frozen: d["fee_router_frozen"].as_bool().unwrap(),
    })
}

#[tokio::test]
async fn recorded_frame_survives_repeats_and_only_its_dex_limit_changes_retention() {
    let rows = archive(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum.json.gz"));
    let (roles, frame) = recorded(&rows);
    let at = &rows["selector"];
    let hash = at["blockHash"].as_str().unwrap().parse().unwrap();
    let dir = recovery_tests::directory("motoswap-cache", 1);
    let parts = recovery_tests::parts(&dir, 1, rows["wallet"].as_str().unwrap().parse().unwrap()).await;
    let ctx = &parts.ctx;
    let profiles: Vec<Value> = serde_json::from_str(include_str!("../../../../config/first_run_dex.json")).unwrap();
    let profile = profiles.iter().find(|v| v["settings"]["dex.factory"] == rows["directory"]["roles"]["factory"]).unwrap();
    let f = families::make(profile["id"].as_str().unwrap(), profile["settings"]["dex.kind"].as_str().unwrap()).unwrap();
    let scope = Scope::Dex(f.id.into());
    let cache = ctx.evm_chain.motoswap();
    let cell = || cache.frame(ctx, f.id, hash, roles);

    // Ранее настроенный профиль: параллельные чтения объединены, следующая группа снова проверит роли.
    let first = cell();
    first.get_or_init(|| async { frame }).await;
    assert!(Arc::ptr_eq(&first, &cell()));
    drop(first);
    assert!(cell().get().is_none());
    let recommended = profile["settings"][LIMIT].clone();
    assert_eq!(recommended, 16);
    ctx.settings.set(LIMIT, scope.clone(), Some(recommended), "test").await.unwrap();
    cell().get_or_init(|| async { frame }).await;
    let held = super::super::read(ctx, f, roles, at).await.unwrap();
    assert_eq!((held.lp, held.protocol, held.creator, held.frozen), (frame.lp, frame.protocol, frame.creator, frame.frozen));

    let second_id = "motoswap-ethereum@ethereum";
    assert!(cache.frame(ctx, second_id, hash, roles).get().is_none());
    ctx.settings.set(LIMIT, Scope::Dex(second_id.into()), Some(json!(1)), "test").await.unwrap();
    cache.frame(ctx, second_id, hash, roles).get_or_init(|| async { frame }).await;
    ctx.settings.set(LIMIT, scope.clone(), Some(json!(0)), "test").await.unwrap();
    assert!(cell().get().is_none());
    assert!(cache.frame(ctx, second_id, hash, roles).get().is_some());
    let other = ctx.for_place("bsc_dex");
    assert!(other.evm_chain.motoswap().frame(&other, second_id, hash, roles).get().is_none());
    assert!(ctx.settings.set(LIMIT, scope.clone(), Some(json!(-1)), "test").await.is_err());
    assert!(ctx.settings.set(LIMIT, scope.clone(), Some(json!(1.5)), "test").await.is_err());
    ctx.settings.set(LIMIT, scope, Some(json!(u64::MAX)), "test").await.unwrap();
    cell().get_or_init(|| async { frame }).await;
    assert!(cell().get().is_some());
}

#[tokio::test]
async fn concurrent_version_read_survives_fifo_eviction_and_retries_a_failed_read() {
    let old = archive(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum_wallets.json.gz"));
    let paths = archive(include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/motoswap_ethereum_paths.json.gz"));
    assert_eq!(old["selector"], paths["selector"]);
    assert_eq!(old["module"], paths["module"]);
    let block = paths["selector"]["blockHash"].as_str().unwrap().parse().unwrap();
    let module = paths["module"].as_str().unwrap().parse().unwrap();
    let v1 = old["manifest"]["runtime"].as_str().unwrap().parse().unwrap();
    let v2 = paths["runtime"].as_str().unwrap().parse().unwrap();
    let has_path = paths["payments"].as_array().unwrap().iter().any(|p| p["mode"] == "guarded");
    assert!(has_path);
    let dir = recovery_tests::directory("motoswap-versions", 1);
    let parts = recovery_tests::parts(&dir, 1, paths["owner"].as_str().unwrap().parse().unwrap()).await;
    let ctx = &parts.ctx;
    let id = "motoswap-ethereum";
    let scope = Scope::Dex(id.into());
    let cell = |allowed| ctx.evm_chain.motoswap().version(ctx, id, block, module, allowed);
    ctx.settings.set(LIMIT, scope.clone(), Some(json!(1)), "test").await.unwrap();
    let first = cell(vec![v2]);
    let (started, waiting) = tokio::sync::oneshot::channel();
    let (resume, release) = tokio::sync::oneshot::channel();
    let read = tokio::spawn(async move {
        *first.get_or_init(|| async move {
            started.send(()).unwrap();
            release.await.unwrap();
            has_path
        }).await
    });
    waiting.await.unwrap();
    // Новый список хешей вытесняет запись до завершения первого чтения.
    let alternate = cell(vec![v1, v2]);
    assert!(alternate.get().is_none());
    drop(alternate);
    let repeat = cell(vec![v2]);
    resume.send(()).unwrap();
    assert!(read.await.unwrap());
    assert_eq!(repeat.get(), Some(&has_path));
    drop(repeat);
    let next = cell(vec![v1, v2]);
    assert!(next.get_or_try_init(|| async { Err::<bool, _>("ошибка чтения") }).await.is_err());
    assert_eq!(next.get_or_init(|| async { has_path }).await, &has_path);
    let oldest = Arc::downgrade(&next);
    drop(next);
    ctx.settings.set(LIMIT, scope.clone(), Some(json!(2)), "test").await.unwrap();
    let newest = cell(vec![v2]);
    assert!(newest.get().is_none());
    newest.get_or_init(|| async { has_path }).await;
    drop(newest);
    assert!(oldest.upgrade().is_some());
    ctx.settings.set(LIMIT, scope.clone(), Some(json!(1)), "test").await.unwrap();
    assert_eq!(cell(vec![v2]).get(), Some(&has_path));
    assert!(oldest.upgrade().is_none());
    ctx.settings.set(LIMIT, scope, None, "test").await.unwrap();
    assert!(cell(vec![v2]).get().is_none());
}
