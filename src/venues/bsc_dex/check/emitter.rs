//! Файл: venues/bsc_dex/check/emitter.rs
//! Что делает: проверка «кто меняет» (решение 59). Universal Router сам находит пару по своей фабрике — адрес
//! проверенного пула ему не передаётся. Поэтому в пробном обмене (eth_simulateV1) и в пробе сделки перед отправкой
//! смотрим, КТО выпустил событие обмена: V2/V3 — адрес контракта события = адрес пула; V4/Infinity — менеджер пула
//! и topics[1] = PoolId. Нет такого события у пула маршрута — пул исключается с причиной «Universal Router DEX X
//! меняет в другом пуле» (справочник — «отклонён», котировка его больше не берёт, журнал — один раз).
//! Кто вызывает: check/probe.rs, exec/precheck_sim.rs.
//! От чего зависит: check/simulate.rs (SimLog), pools/{abi_v2v3,abi_v4,registry}.rs, directory (ban), ctx.rs.
//! Настройки: нет.
//!
//! События обмена — официальные ABI: V2 Swap — IUniswapV2Pair.sol#L26-L33 (v2-core 6a9e7c97); V3 Swap — PancakeSwap
//! IPancakeV3PoolEvents.sol#L74-L84, Uniswap IUniswapV3PoolEvents.sol#L72-L80; V4 Swap (id indexed) — ICLPoolManager.sol,
//! IBinPoolManager.sol (infinity-core 3c5ef1be), Uniswap IPoolManager.sol (v4-core bea7f6d1) — pools/abi_*.rs.

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use crate::core::types::PoolRef;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::pools::abi_v2v3::{IPancakeV3Pool, IUniswapV3Pool, IV2Pair};
use super::super::pools::abi_v4::{IInfinityBin, IInfinityCl, IUniswapV4};
use super::super::pools::registry::Entry;
use super::simulate::SimLog;

/// topic0 всех известных событий обмена. Ошибок нет.
pub fn swap_topics() -> [B256; 6] {
    [
        IV2Pair::Swap::SIGNATURE_HASH,
        IPancakeV3Pool::Swap::SIGNATURE_HASH,
        IUniswapV3Pool::Swap::SIGNATURE_HASH,
        IInfinityCl::Swap::SIGNATURE_HASH,
        IInfinityBin::Swap::SIGNATURE_HASH,
        IUniswapV4::Swap::SIGNATURE_HASH,
    ]
}

/// Выпустил ли этот пул событие обмена. Ошибок нет.
pub fn by_pool(e: &Entry, log: &SimLog) -> bool {
    let swap = log
        .topics
        .first()
        .is_some_and(|t| swap_topics().contains(t));
    swap && match &e.pool.reference {
        PoolRef::Contract(a) => log.address == *a,
        PoolRef::Singleton { manager, id } => {
            log.address == *manager && log.topics.get(1) == Some(id)
        }
    }
}

/// Пулы маршрута без своего события обмена. Принимает: пулы маршрута, события вызова обмена.
/// Возвращает: [(номер пула, причина)] — пусто, если каждый пул сам выпустил событие обмена. Ошибок нет.
pub fn missing(entries: &[&Entry], logs: &[SimLog]) -> Vec<(usize, String)> {
    let others: Vec<String> = logs
        .iter()
        .filter(|l| l.topics.first().is_some_and(|t| swap_topics().contains(t)))
        .filter(|l| !entries.iter().any(|e| by_pool(e, l)))
        .map(|l| format!("{}", l.address))
        .collect();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !logs.iter().any(|l| by_pool(e, l)))
        .map(|(i, e)| {
            let from = if others.is_empty() {
                "события обмена от пула нет".to_string()
            } else {
                format!("событие обмена — от {}", others.join(", "))
            };
            let why = format!(
                "Universal Router DEX {} меняет в другом пуле: {from}, а пул маршрута {:?}",
                e.family.id, e.pool.reference
            );
            (i, why)
        })
        .collect()
}

/// Проверка событий вызова обмена: пулы без своего события исключаются (справочник, журнал — один раз).
/// Возвращает: Ok или причины через «; ». Ошибок нет (причина — текст).
pub async fn check(ctx: &Ctx, entries: &[&Entry], logs: &[SimLog]) -> Result<(), String> {
    let miss = missing(entries, logs);
    if miss.is_empty() {
        return Ok(());
    }
    let mut all = Vec::new();
    for (i, why) in miss {
        if ctx.index.ban(&entries[i].pool.reference, &why) {
            let msg = format!("Пул исключён: {why}");
            ctx.log(Level::Error, "dex.check", msg, None).await;
        }
        all.push(why);
    }
    Err(all.join("; "))
}
