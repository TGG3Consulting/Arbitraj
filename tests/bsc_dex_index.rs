//! Файл: tests/bsc_dex_index.rs
//! Что делает: пулы из справочника индексатора → пулы подписки (решение 58) без сети: таблица сопоставления
//! «DEX индексатора → вид» (не подключена, не поддерживается, выключена, не хватает адреса), итог по пулам CAKE из
//! живого ответа GeckoTerminal 2026-10-24, проверка пула на узле — чистые решения по ответам: чужая фабрика, чужие
//! токены (пул-подделка «比特币 / Cake»), пул не в реестре фабрики, своя комиссия пары Biswap (swapFee / 1000), ключ
//! пула Infinity у менеджера (PoolId, менеджер, валюты).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools, directory}, arb::settings (настоящая SQLite во временной папке).
//! ЖДУТ ЗАПИСИ: ответов узла BSC в записи пока нет (из песочницы узлы недоступны) — тесты решений проверки на узле
//! (wrong_factory_or_tokens_rejected, v3_abi_by_lm_pool, infinity_key_from_manager) собраны из адресов записи индексатора
//! и официальных страниц адресов; список точных вызовов для записи — tests/data/node_calls_2026-10-24.json. После записи
//! техлидом эти тесты переводятся на записанные ответы.

use std::collections::HashSet;
use std::str::FromStr;

use alloy::sol_types::SolValue;
use alloy_primitives::{Address, B256, Bytes, U256, address};
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
use arb::venues::bsc_dex::pools::key::{InfinityPoolKeyData, PoolKey};
use arb::venues::bsc_dex::pools::verify::{self, Candidate, Read};
use arb::venues::bsc_dex::pools::{index, verify_v4};

const CAKE: Address = address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");
const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");
/// PancakeSwap V3 Factory и Uniswap V3 Factory (BNB) — официальные страницы адресов (indexer_research.md, 2.1).
const PCS_V3_FACTORY: Address = address!("0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
const UNI_V3_FACTORY: Address = address!("0xdB1d10011AD0Ff90774D0C6Bb92e5C5c8b4461F7");
/// PancakeSwap Infinity CLPoolManager (developer.pancakeswap.finance, адреса Infinity).
const CL_MANAGER: Address = address!("0xa0FfB9c1CE1Fe56963B0321B32E7A0302114058b");

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
    // Форк (Biswap — V2 со своей фабрикой, без Universal Router своей фабрики): в сделки не идёт (решение 59).
    set(
        &c,
        &[("dex.kind", Scope::Dex("biswap".into()), json!("v2_fork"))],
    )
    .await;
    let e = families::classify(&c, "biswap").unwrap_err();
    assert!(e.starts_with("DEX biswap не подключена к сделкам"), "{e}");
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

/// Кандидат: пул CAKE/USDT PancakeSwap V3 из записи GeckoTerminal.
fn cand() -> Candidate {
    Candidate {
        family: families::make("pancakeswap-v3-bsc", "pancakeswap_v3").unwrap(),
        pool: "0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1".into(),
        coin: CAKE,
        quote: USDT,
    }
}

#[test]
fn wrong_factory_or_tokens_rejected() {
    let (t0, t1) = if CAKE < USDT {
        (CAKE, USDT)
    } else {
        (USDT, CAKE)
    };
    let ok = Read {
        factory: PCS_V3_FACTORY,
        token0: t0,
        token1: t1,
        fee: Some(2500),
        spacing: Some(50),
        v2_fee: None,
    };
    assert!(verify::check(&cand(), PCS_V3_FACTORY, &ok).is_ok());
    let alien = Read {
        factory: UNI_V3_FACTORY,
        ..ok.clone()
    };
    let why = verify::check(&cand(), PCS_V3_FACTORY, &alien).unwrap_err();
    assert!(why.starts_with("factory() пула ="), "{why}");
    // Пул-подделка «比特币 / Cake» из записи (резерв $7,7 млн по индексатору): токен 0xb595… — не USDT.
    let fake = address!("0xb595a01086043ff14babdf0c41191d5163777777");
    let bad = Read {
        token0: fake.min(CAKE),
        token1: fake.max(CAKE),
        ..ok
    };
    assert!(
        verify::check(&cand(), PCS_V3_FACTORY, &bad)
            .unwrap_err()
            .starts_with("токены пула")
    );
    // Реестр фабрики: getPool возвращает тот же адрес — пул её; другой — нет.
    let pool = Address::from_str(&cand().pool).unwrap();
    assert!(verify::registered(pool, &pool.abi_encode()).is_ok());
    assert!(
        verify::registered(pool, &Address::ZERO.abi_encode())
            .unwrap_err()
            .contains("не знает")
    );
}

#[test]
fn v3_abi_by_lm_pool() {
    let pcs = families::make("pancakeswap-v3-bsc", "pancakeswap_v3").unwrap();
    let uni = families::make("uniswap-bsc", "uniswap_v3").unwrap();
    let lm = Ok(Bytes::from(Address::ZERO.abi_encode()));
    let none: Result<Bytes, String> = Err("откат".into());
    assert!(verify::v3_abi(pcs, Some(&lm)).is_ok());
    assert!(
        verify::v3_abi(pcs, Some(&none))
            .unwrap_err()
            .contains("не пул PancakeSwap V3")
    );
    assert!(verify::v3_abi(uni, Some(&none)).is_ok());
    assert!(
        verify::v3_abi(uni, Some(&lm))
            .unwrap_err()
            .contains("это пул PancakeSwap V3")
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

#[test]
fn infinity_key_from_manager() {
    let f = families::make("pancakeswap-infinity-clmm", "pancakeswap_infinity_cl").unwrap();
    let (c0, c1) = if CAKE < USDT {
        (CAKE, USDT)
    } else {
        (USDT, CAKE)
    };
    let params = B256::from(U256::from(10u32) << 16); // tickSpacing 10, без флагов крючка
    let key = InfinityPoolKeyData {
        currency0: c0,
        currency1: c1,
        hooks: Address::ZERO,
        manager: CL_MANAGER,
        fee: 500,
        parameters: params,
    };
    let id = PoolKey::Infinity(key.clone()).id();
    let answer = |m: Address| {
        Ok(Bytes::from(
            (c0, c1, Address::ZERO, m, U256::from(500u32), params).abi_encode_params(),
        ))
    };
    let c = Candidate {
        family: f,
        pool: format!("{id}"),
        coin: CAKE,
        quote: USDT,
    };
    let found = verify_v4::decide(
        &c,
        id,
        CL_MANAGER,
        answer(CL_MANAGER),
        Err("не нужен".into()),
    )
    .unwrap();
    assert_eq!(
        (found.coin, found.tick_spacing, found.fee_pips),
        (CAKE, Some(10), Some(500))
    );
    // Менеджер не знает PoolId — в ответе нулевой менеджер.
    let why = verify_v4::decide(
        &c,
        id,
        CL_MANAGER,
        answer(Address::ZERO),
        Err("не нужен".into()),
    )
    .unwrap_err();
    assert!(why.starts_with("менеджер не знает этот PoolId"), "{why}");
    // Чужой PoolId с тем же ключом — отказ.
    let other = B256::repeat_byte(7);
    assert!(
        verify_v4::decide(
            &c,
            other,
            CL_MANAGER,
            answer(CL_MANAGER),
            Err(String::new())
        )
        .unwrap_err()
        .contains("PoolId")
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

/// Запись реестра: пара CAKE/USDT PancakeSwap V2 из записи GeckoTerminal.
fn v2_entry() -> arb::venues::bsc_dex::pools::registry::Entry {
    use arb::core::types::{Pool, PoolRef, PoolState, VenueId};
    let f = families::make("pancakeswap_v2", "pancakeswap_v2").unwrap();
    let addr = address!("0xa39af17ce4a8eb807e076805da1e2b8ea7d0755b");
    arb::venues::bsc_dex::pools::registry::Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(addr),
            token0: CAKE.min(USDT),
            token1: CAKE.max(USDT),
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::ZERO,
                reserve1: U256::ZERO,
            },
            block: 1,
            received_at: chrono::Utc::now(),
        },
        ticker: "CAKE".into(),
        coin: CAKE,
        usdt: USDT,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

#[test]
fn who_swaps() {
    use arb::venues::bsc_dex::check::emitter;
    use arb::venues::bsc_dex::check::simulate::SimLog;
    let e = v2_entry();
    let swap = emitter::swap_topics()[0];
    let ours = SimLog {
        address: address!("0xa39af17ce4a8eb807e076805da1e2b8ea7d0755b"),
        topics: vec![swap],
    };
    // Пара PancakeSwap V1 CAKE/WBNB из той же записи — «другой пул».
    let other = SimLog {
        address: address!("0xa527a61703d82139f8a06bc30097cc9caa2df5a6"),
        topics: vec![swap],
    };
    assert!(emitter::missing(&[&e], std::slice::from_ref(&ours)).is_empty());
    let miss = emitter::missing(&[&e], std::slice::from_ref(&other));
    assert_eq!(miss.len(), 1);
    assert!(
        miss[0]
            .1
            .starts_with("Universal Router DEX pancakeswap_v2 меняет в другом пуле"),
        "{}",
        miss[0].1
    );
}
