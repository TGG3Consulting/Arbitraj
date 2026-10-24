//! Файл: tests/bsc_dex_events.rs
//! Что делает: проверяет порядок применения событий пула DEX: несколько событий в одном блоке применяются все и
//! по порядку logIndex; повтор уже учтённого события и события блоков, учтённых чтением состояния, отбрасываются.
//! Кто вызывает: cargo test.
//! От чего зависит: venues/bsc_dex/pools/{registry,events,abi_v2v3,families}.rs.
//!
//! Лог собирается в формате ответа узла (поля Log — execution-apis src/schemas/receipt.yaml), данные события —
//! кодированием официального ABI Sync(uint112,uint112) (UniswapV2Pair.sol#L73-L86). Записанных логов BSC нет:
//! из песочницы узлы BSC недоступны — сама логика порядка от содержимого событий не зависит.
//! В V2 каждое изменение пары (Swap, Mint, Burn) заканчивается событием Sync с новыми резервами, поэтому
//! «несколько Swap/Mint в блоке» для пары — это несколько Sync в блоке.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, U256, address, aliases::U112};
use chrono::Utc;

use arb::core::types::{Pool, PoolRef, PoolState, VenueId};
use arb::venues::bsc_dex::pools::abi_v2v3::IV2Pair;
use arb::venues::bsc_dex::pools::events::{self, Applied};
use arb::venues::bsc_dex::pools::families::FAMILIES;
use arb::venues::bsc_dex::pools::registry::Entry;

const PAIR: Address = address!("0x1000000000000000000000000000000000000001");

/// Лог Sync в блоке с logIndex.
fn sync(block: u64, index: u64, r0: u64, r1: u64) -> Log {
    let ev = IV2Pair::Sync {
        reserve0: U112::from(r0),
        reserve1: U112::from(r1),
    };
    let data = ev.encode_log_data();
    serde_json::from_value(serde_json::json!({
        "address": PAIR,
        "topics": data.topics(),
        "data": data.data,
        "blockNumber": format!("{block:#x}"),
        "blockHash": format!("{:#066x}", block),
        "transactionHash": format!("{:#066x}", index + 1),
        "transactionIndex": "0x0",
        "logIndex": format!("{index:#x}"),
        "removed": false
    }))
    .expect("лог в формате узла")
}

/// Запись пары V2, прочитанная на конце блока `block`.
fn entry(block: u64) -> Entry {
    let family = FAMILIES.iter().find(|f| f.id == "pancakeswap_v2").unwrap();
    Entry {
        family,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: family.id.into(),
            kind: family.kind,
            reference: PoolRef::Contract(PAIR),
            token0: Address::ZERO,
            token1: PAIR,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::from(1u8),
                reserve1: U256::from(1u8),
            },
            block,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin: PAIR,
        usdt: Address::ZERO,
        key: None,
        ticks: None,
        bins: None,
        synced_block: block,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

/// Как listen_log::on_log: учтённое отбрасывается, новое применяется.
fn feed(e: &mut Entry, log: &Log) -> bool {
    if e.has_applied(log.block_number.unwrap(), log.log_index.unwrap()) {
        return false;
    }
    events::apply(e, log) == Applied::Updated
}

fn reserves(e: &Entry) -> (U256, U256) {
    match e.pool.state {
        PoolState::V2 { reserve0, reserve1 } => (reserve0, reserve1),
        _ => unreachable!(),
    }
}

#[test]
fn all_events_of_one_block_apply_in_log_index_order() {
    let mut e = entry(100);
    // Событие блока, уже учтённого чтением состояния, — отбрасывается.
    assert!(!feed(&mut e, &sync(100, 9, 5, 5)));
    // Три события в блоке 101 — все применяются.
    assert!(feed(&mut e, &sync(101, 2, 10, 20)));
    assert!(feed(&mut e, &sync(101, 4, 11, 19)));
    assert!(feed(&mut e, &sync(101, 7, 12, 18)));
    assert_eq!(reserves(&e), (U256::from(12u8), U256::from(18u8)));
    assert_eq!((e.synced_block, e.synced_log), (101, Some(7)));
    // Повтор уже применённого события того же блока — отбрасывается, состояние не откатывается.
    assert!(!feed(&mut e, &sync(101, 4, 11, 19)));
    assert_eq!(reserves(&e), (U256::from(12u8), U256::from(18u8)));
    // Следующий блок, первое событие — применяется.
    assert!(feed(&mut e, &sync(102, 0, 13, 17)));
    assert_eq!(reserves(&e), (U256::from(13u8), U256::from(17u8)));
}
