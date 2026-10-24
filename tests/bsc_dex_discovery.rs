//! Файл: tests/bsc_dex_discovery.rs
//! Что делает: проверяет части поиска пулов без истории блоков: блок развёртывания менеджера V4 переживает
//! перезапуск (SQLite, storage/venue_state.rs) и привязан к адресу менеджера; событие создания пула запускает
//! поиск только для монет подписки; пул V4 выбирается по встречному токену (USDT или промежуточная монета).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::{deploy_block, created, discover}, arb::storage (настоящая SQLite).
//!
//! Лог PoolCreated собирается в формате ответа узла (execution-apis src/schemas/receipt.yaml) кодированием
//! официального ABI (IPancakeV3Factory.sol#L23-L29). Записанных логов BSC нет: из песочницы узлы BSC недоступны;
//! адреса — из официальных страниц (research/flow/bsc_dex.md, раздел 1: фабрика PancakeSwap V3, WBNB, USDT).

use std::collections::HashSet;

use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, address, aliases::I24, aliases::U24};
use tokio::sync::broadcast;

use arb::core::types::PoolRef;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::storage::venue_state;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::abi_v2v3::IV3Factory;
use arb::venues::bsc_dex::pools::discover::{Found, against};
use arb::venues::bsc_dex::pools::families::FAMILIES;
use arb::venues::bsc_dex::pools::{created, deploy_block};

const FACTORY: Address = address!("0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
const USDT: Address = address!("0x55d398326f99059fF775485246999027B3197955");
const WBNB: Address = address!("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");

/// Окружение на базе в папке (та же папка — «перезапуск»).
async fn ctx(d: &std::path::Path) -> Ctx {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

#[tokio::test]
async fn deploy_block_survives_restart_per_manager() {
    let d = std::env::temp_dir().join(format!("arb-dex-deploy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let f = FAMILIES.iter().find(|f| f.id == "uniswap_v4").unwrap();
    {
        let c = ctx(&d).await;
        assert_eq!(deploy_block::stored(&c, f, FACTORY).await, None);
        let id = format!("{}:{FACTORY:#x}", f.id);
        venue_state::save(
            c.journal.db(),
            "bsc_dex",
            "deploy_block",
            &id,
            "45970616".into(),
        )
        .await
        .unwrap();
    }
    let c = ctx(&d).await;
    assert_eq!(deploy_block::stored(&c, f, FACTORY).await, Some(45970616));
    // Другой адрес менеджера — другой блок (ищется заново).
    assert_eq!(deploy_block::stored(&c, f, WBNB).await, None);
    let _ = std::fs::remove_dir_all(&d);
}

/// Лог PoolCreated(token0, token1, fee, tickSpacing, pool) фабрики PancakeSwap V3 в формате узла.
fn pool_created(token0: Address, token1: Address) -> String {
    let ev = IV3Factory::PoolCreated {
        token0,
        token1,
        fee: U24::from(2500),
        tickSpacing: I24::try_from(50).unwrap(),
        pool: address!("0x1000000000000000000000000000000000000001"),
    };
    let data = ev.encode_log_data();
    serde_json::json!({
        "address": FACTORY,
        "topics": data.topics(),
        "data": data.data,
        "blockNumber": "0x1", "blockHash": format!("{:#066x}", 1), "transactionHash": format!("{:#066x}", 2),
        "transactionIndex": "0x0", "logIndex": "0x0", "removed": false
    })
    .to_string()
}

#[test]
fn creation_event_matters_only_for_our_coins() {
    let mine: HashSet<B256> = [B256::left_padding_from(WBNB.as_slice())].into();
    assert!(created::relevant(&pool_created(USDT, WBNB), &mine));
    let other = address!("0x2000000000000000000000000000000000000002");
    assert!(!created::relevant(&pool_created(USDT, other), &mine));
    // Не разобран — поиск всё равно запускается (пропуск пула хуже лишнего поиска).
    assert!(created::relevant("{}", &mine));
}

#[test]
fn v4_pool_is_taken_against_quote_token() {
    let f = FAMILIES.iter().find(|f| f.id == "uniswap_v4").unwrap();
    let p = Found {
        family: f,
        reference: PoolRef::Contract(FACTORY),
        token0: USDT,
        token1: WBNB,
        coin: USDT,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    };
    assert_eq!(against(&p, USDT).unwrap().coin, WBNB);
    assert_eq!(against(&p, WBNB).unwrap().coin, USDT);
    assert!(against(&p, FACTORY).is_none());
}
