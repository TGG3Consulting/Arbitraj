//! Файл: tests/bsc_dex_index.rs
//! Что делает: пулы из справочника индексатора → пулы подписки (решение 58) без сети: таблица сопоставления
//! «DEX индексатора → вид» (не подключена, не поддерживается, выключена, не хватает адреса), итог по пулам CAKE из
//! живого ответа GeckoTerminal 2026-10-24, проверка пула на узле — чистые решения по ответам: чужая фабрика, чужие
//! токены (пул-подделка «比特币 / Cake»), пул не в реестре фабрики, своя комиссия пары Biswap (swapFee / 1000), ключ
//! пула Infinity у менеджера (PoolId, менеджер, валюты).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools, directory}, arb::settings (настоящая SQLite во временной папке).
//! Ответов узла BSC в записи нет (из песочницы узлы недоступны): «прочитанное с пула» задано адресами из записи
//! индексатора и официальных страниц адресов (research: PancakeSwap V3 Factory, Uniswap V3 Factory, CLPoolManager).

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

/// Все настройки DEX вида V3 PancakeSwap, кроме `skip`.
fn v3_row(id: &str, skip: &str) -> Vec<(&'static str, Scope, Value)> {
    let a = json!("0x1111111111111111111111111111111111111111");
    let s = Scope::Dex(id.into());
    let all = vec![
        ("dex.kind", s.clone(), json!("v3_fork_pancake")),
        ("dex.pool_enabled", s.clone(), json!(true)),
        ("dex.universal_router", s.clone(), a.clone()),
        (
            "dex.universal_router_version",
            s.clone(),
            json!("pancakeswap_infinity"),
        ),
        ("dex.permit2", s.clone(), a.clone()),
        ("dex.factory", s.clone(), json!(format!("{PCS_V3_FACTORY}"))),
        ("dex.tick_lens", s.clone(), a),
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
    set(&c, &v3_row("pancakeswap-v3-bsc", "dex.tick_lens")).await;
    let e = families::classify(&c, "pancakeswap-v3-bsc").unwrap_err();
    assert!(e.ends_with("не задана dex.tick_lens"), "{e}");
    set(&c, &v3_row("pancakeswap-v3-bsc", "")).await;
    let f = families::classify(&c, "pancakeswap-v3-bsc").unwrap();
    assert_eq!((f.id, f.proto), ("pancakeswap-v3-bsc", Proto::PancakeV3));
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
    // Списка DEX в коде нет: виды — из настроек (unsupported в список видов не входит).
    let ids: Vec<&str> = families::all(&c).iter().map(|f| f.id).collect();
    assert_eq!(ids, vec!["pancakeswap-v3-bsc"]);
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
    // Порог не задан — проверки нет вовсе (не задано — не работает).
    assert!(index::take(&c, None).await.is_err());
    set(
        &c,
        &[(
            "index.min_reserve_usd",
            Scope::Venue("bsc_dex".into()),
            json!("1000000000"),
        )],
    )
    .await;
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
    // Кандидатов нет (всё мельче порога или не подключено) — узел не нужен.
    let taken = index::take(&c, None).await.unwrap();
    assert!(taken.is_empty());
    let st = |pool: &str| c.index.with(|s| s.status.get(pool).cloned().unwrap());
    assert_eq!(
        st("0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1"),
        DirectoryStatus::Small
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
    assert_eq!(v.pools.len(), 6, "CAKE/USDT — шесть пулов в записи");
    assert!(
        v.pools
            .iter()
            .all(|p| p.status != DirectoryStatus::Unchecked)
    );
}

/// Кандидат: пул CAKE/USDT PancakeSwap V3 из записи GeckoTerminal.
fn cand() -> Candidate {
    Candidate {
        family: families::make("pancakeswap-v3-bsc", "v3_fork_pancake").unwrap(),
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
fn biswap_pair_fee() {
    // BiswapPair.sol: swapFee = 1 по умолчанию (0,1%), максимум 1000; формула balance·1000 − amountIn·swapFee.
    assert_eq!(verify::v2_fee_pips(U256::from(1u8), 1000).unwrap(), 1000);
    assert_eq!(verify::v2_fee_pips(U256::from(2u8), 1000).unwrap(), 2000);
    assert!(verify::v2_fee_pips(U256::from(1000u32), 1000).is_err());
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
