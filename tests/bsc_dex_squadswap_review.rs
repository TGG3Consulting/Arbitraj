//! Файл: tests/bsc_dex_squadswap_review.rs
//! Что проверяет: доработки по ревью решения 281. В1 — dex.v3_fee_source меняют на лету, пока пачка голов V3 в пути. Признак добавок головы
//! (fee_fresh::Extras: swapFee 274, fee() 281) вычисляется ОДИН раз при сборке пачки и тем же значением разбирается
//! ответ (pools/load_batch.rs). На РЕАЛЬНЫХ ответах узла BSC (блок 133618816, tests/data/squadswap_v3_2026-11-15/
//! probe.json, пулы SquadSwap V3):
//! 1. Пачка собрана без настройки (2 вызова), к разбору включили pool_fee_each_read — разбор по признаку сборки: тот
//!    же итог, что без настройки, паники нет.
//! 2. Пачка собрана с pool_fee_each_read (3 вызова: slot0, liquidity, fee()), к разбору настройку сменили на
//!    factory_swap_fee — fee() отделяется по признаку сборки и не принимается за ответ swapFee (ложного отказа 274 нет).
//! 3. Защита: ответов головы меньше нужного — ошибка «пул не прочитан», а не выход за границу.
//!
//! В3 — неполная настройка (fee_probe::plan): менеджер комиссии без pool_fee_each_read — пулы DEX не берутся; проба
//! swapFee(0x0, 0) пропускается только при factory_swap_fee. В4 — dex.v3_swap_callback (exec_callback::missing):
//! squadV3SwapCallback нет в коде исполнителя v1 (contracts/out/v1/, версия сверена у кошелька — задача t114) — причина
//! «нужен исполнитель новой версии»; пул без
//! пути исполнения даёт «не проверено» (pool_taxes::outcome → Wait), а не «проверка не прошла».
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{ctx, check::{pool_taxes, taxes}, pools::{exec_callback, exec_kind, families,
//! fee_fresh, fee_probe, load_batch, load_batch_v2v3, verify}}, arb::settings
//! (настоящая SQLite во временной папке).

use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::PoolRef;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::check::{pool_taxes, taxes::Outcome};
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::verify::Verdict;
use arb::venues::bsc_dex::pools::{exec_callback, exec_kind, fee_probe};
use arb::venues::bsc_dex::pools::fee_fresh::{self, Extras};
use arb::venues::bsc_dex::pools::load_batch::Want;
use arb::venues::bsc_dex::pools::load_batch_v2v3::{self as v23, PoolStateTick};

const DEX: &str = "squadswap-v3-bsc";

fn probe() -> Value {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/squadswap_v3_2026-11-15/probe.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn bytes(v: &Value) -> Bytes {
    Bytes::from(alloy_primitives::hex::decode(v.as_str().unwrap().trim_start_matches("0x")).unwrap())
}

/// Слово `i` ответа (беззнаковое).
fn word(raw: &str, i: usize) -> U256 {
    let h = raw.trim_start_matches("0x");
    U256::from_str_radix(&h[64 * i..64 * (i + 1)], 16).unwrap()
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

/// Номер окружения в процессе: тесты файла идут параллельно — у каждого своя папка (задача t114: общая папка давала
/// «database is locked», когда один тест удалял её, пока другой открывал базу).
static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Окружение на настоящей SQLite (настройки DEX задаёт тест).
async fn ctx() -> Ctx {
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("arb-fee-race-{}-{n}", std::process::id()));
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

async fn set(c: &Ctx, name: &str, v: Option<&str>) {
    let s = Scope::Dex(DEX.into());
    c.settings.set(name, s, v.map(|x| json!(x)), "тест").await.unwrap();
}

async fn set_source(c: &Ctx, v: Option<&str>) {
    set(c, "dex.v3_fee_source", v).await;
}

/// Разбор головы так же, как load_batch.rs: fee() — по признаку сборки, остаток — parse_head.
fn parse(w: &Want, x: Extras, b: &[Bytes]) -> Result<(PoolStateTick, Option<u32>), String> {
    let (rest, fee) = fee_fresh::split(x.fresh, b).map_err(|e| e.message)?;
    Ok((v23::parse_head(w, rest).map_err(|e| e.message)?, fee))
}

#[tokio::test]
async fn setting_changed_between_build_and_parse() {
    let c = ctx().await;
    let f = families::make(DEX, "v3_fork_pancake").unwrap();
    let d = probe();
    for p in d["pools"].as_array().unwrap() {
        let k = &p["calls"];
        let r = PoolRef::Contract(addr(&p["pool"]));
        let w = Want {
            family: f,
            reference: &r,
            spacing: Some(word(k["tickSpacing"].as_str().unwrap(), 0).to::<u64>() as i32),
            step: None,
            usdt: addr(&p["token1"]),
            token0: addr(&p["token0"]),
            token1: addr(&p["token1"]),
        };
        let fee = word(k["fee"].as_str().unwrap(), 0).to::<u32>();
        let two = vec![bytes(&k["slot0"]), bytes(&k["liquidity"])];
        let three = vec![bytes(&k["slot0"]), bytes(&k["liquidity"]), bytes(&k["fee"])];
        // 1. Сборка без настройки, разбор — уже с pool_fee_each_read.
        set_source(&c, None).await;
        let x0 = Extras::of(&c, f);
        assert_eq!(x0, Extras::default());
        assert_eq!(v23::head(&c, &w, x0).unwrap().len(), 2);
        set_source(&c, Some("pool_fee_each_read")).await;
        assert!(Extras::of(&c, f).fresh, "настройка уже включена");
        let (h, got) = parse(&w, x0, &two).expect("разбор по признаку сборки");
        assert_eq!(got, None);
        let (h0, _) = parse(&w, Extras::default(), &two).unwrap();
        match (h, h0) {
            (PoolStateTick::Ticked(a, t), PoolStateTick::Ticked(b, t0)) => {
                assert_eq!((format!("{a:?}"), t), (format!("{b:?}"), t0))
            }
            _ => panic!("голова V3"),
        }
        // Старый путь (признак разбора перечитан): fee() забрал бы liquidity — теперь ошибка, не паника.
        let stale = Extras { swap_fee: false, fresh: true };
        assert!(parse(&w, stale, &two).is_err());
        // 2. Сборка с pool_fee_each_read, к разбору — factory_swap_fee.
        let x1 = Extras::of(&c, f);
        assert_eq!(v23::head(&c, &w, x1).unwrap().len(), 3);
        set_source(&c, Some("factory_swap_fee")).await;
        assert!(Extras::of(&c, f).swap_fee);
        let (_, got) = parse(&w, x1, &three).expect("fee() отделена по признаку сборки");
        assert_eq!(got, Some(fee));
        // Если бы разбор взял новый признак, fee() ушла бы в проверку swapFee 274 — ложный отказ.
        assert!(parse(&w, Extras { swap_fee: true, fresh: false }, &three).is_err());
        // 3. Защита длины.
        let why = v23::parse_head(&w, &two[..1]).err().unwrap().message;
        assert!(why.contains("пул не прочитан"), "{why}");
        assert!(v23::parse_head(&w, &[]).is_err());
    }
}

#[tokio::test]
async fn incomplete_setup_and_missing_executor_callback() {
    let c = ctx().await;
    let f = families::make(DEX, "v3_fork_pancake").unwrap();
    let d = probe();
    set(&c, "dex.factory", Some(d["factory"].as_str().unwrap())).await;
    // В3: ключ реестра задан, источник — нет: пулы не берутся с причиной; с pool_fee_each_read — проба swapFee идёт.
    set(&c, "dex.v3_registry_fee_method", Some("defaultFee()")).await;
    let (p, asks) = fee_probe::plan(&c, [f].iter());
    assert!(asks.is_empty());
    assert!(matches!(p.check(DEX), Err(Verdict::Rejected(w)) if w.contains("pool_fee_each_read")));
    set_source(&c, Some("pool_fee_each_read")).await;
    let (p, asks) = fee_probe::plan(&c, [f].iter());
    assert_eq!(asks.len(), 1, "проба swapFee не пропускается при pool_fee_each_read (М5)");
    assert!(matches!(p.check(DEX), Err(Verdict::Failed(_))), "ждёт ответа фабрики");
    set_source(&c, Some("factory_swap_fee")).await;
    set(&c, "dex.v3_registry_fee_method", None).await;
    let (p, asks) = fee_probe::plan(&c, [f].iter());
    assert!(asks.is_empty() && p.check(DEX).is_ok());
    // В4: обратный вызов Squad — нет в коде исполнителя v1; PancakeSwap V3 — есть; не задан — проверки нет. Задача
    // t114: проверка — по версии, сверенной у кошелька (v2 и «не сверен» — tests/bsc_dex_squadswap_exec_v2.rs).
    let a = c.settings.accounts();
    let (id, x) = (a.reserve_id().await.unwrap(), "0x00000000000000000000000000000000000000ee");
    a.add(&id, "тест", "wallet.test.key", "bsc_dex", "тест").await.unwrap();
    a.set_executor(&id, Some(x.into()), "тест").await.unwrap();
    a.map_set(arb::settings::accounts::ROW_PROBE, Some(&id), "тест").await.unwrap();
    a.set_executor_version(&id, x, Some("v1".into()), None, "тест").await.unwrap();
    assert_eq!(exec_callback::missing(&c, f), None);
    set(&c, "dex.v3_swap_callback", Some("squadV3SwapCallback(int256,int256,bytes)")).await;
    let why = exec_callback::missing(&c, f).expect("вызова нет в коде исполнителя v1");
    assert!(why.contains("0xc4e21d86") && why.contains("нужен исполнитель новой версии"), "{why}");
    assert!(!exec_kind::via_executor(&c, f));
    assert!(exec_kind::describe(&c, f).contains("без пути исполнения"));
    set(&c, "dex.v3_swap_callback", Some("pancakeV3SwapCallback(int256,int256,bytes)")).await;
    assert_eq!(exec_callback::missing(&c, f), None);
    // Пул без пути исполнения — помеха (NoRoute → env), не вердикт: монета «не проверена», не в чёрный список.
    let out = pool_taxes::outcome(&[], false, &[], (Vec::new(), vec![why], Vec::new()), &[]);
    assert!(matches!(out, Outcome::Wait(_)), "{out:?}");
}
