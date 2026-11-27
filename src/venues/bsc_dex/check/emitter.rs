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
    if let crate::core::types::PoolState::Brokerbot(s)=&e.pool.state {return super::brokerbot_probe::traded(s,log);}
    if e.family.variant.umbrae(){return e.pool.reference.address()==log.address && log.topics.first()==Some(&super::super::pools::umbrae::swap_topic(e.family.variant));}
    if let crate::core::types::PoolState::Temple(s)=&e.pool.state {
        return log.address==s.pair && log.topics.first()==Some(&IV2Pair::Swap::SIGNATURE_HASH);
    }
    if let crate::core::types::PoolState::BancorLegacy(s)=&e.pool.state {
        use super::super::exec::bancor_legacy_wallet::abi::ILegacyNetwork;
        return log.address==s.network && log.topics.len()==4 && log.topics[0]==ILegacyNetwork::Conversion::SIGNATURE_HASH &&
            s.tokens.iter().filter_map(|input|s.path(*input)).any(|path|path.windows(3).step_by(2)
                .any(|leg|log.topics[1]==leg[1].into_word() && log.topics[2]==leg[0].into_word() && log.topics[3]==leg[2].into_word()));
    }
    if e.family.variant==super::super::pools::family_variant::Variant::YieldBasis {
        use super::super::exec::yield_basis_wallet::abi::{IUnderlyingSwap,IVirtualSwap};
        let PoolRef::Singleton{id,..}=&e.pool.reference else{return false;};
        let Some((_,role,venue))=super::super::pools::yield_basis::key(*id) else{return false;};
        let topic=if role==2 {IVirtualSwap::TokenExchange::SIGNATURE_HASH}else{IUnderlyingSwap::TokenExchange::SIGNATURE_HASH};
        return log.address==venue && log.topics.len()==2 && log.topics[0]==topic;
    }
    if let crate::core::types::PoolState::BalancerV1(s)=&e.pool.state {
        use super::super::exec::balancer_v1_wallet::IBPoolWallet;
        return log.address==s.pool && log.topics.len()==4 && log.topics[0]==IBPoolWallet::LOG_SWAP::SIGNATURE_HASH &&
            [(e.pool.token0,e.pool.token1),(e.pool.token1,e.pool.token0)].into_iter()
                .any(|(a,b)| log.topics[2]==a.into_word() && log.topics[3]==b.into_word());
    }
    if let crate::core::types::PoolState::BancorV3(s) = &e.pool.state {
        use super::super::exec::bancor_v3_wallet::IBancorTrade;
        return log.address == s.network && log.topics.len() == 4 &&
            log.topics[0] == IBancorTrade::TokensTraded::SIGNATURE_HASH &&
            s.physical.iter().enumerate().any(|(i, source)| log.topics[2] == source.into_word() &&
                log.topics[3] == s.physical[1-i].into_word());
    }
    if let crate::core::types::PoolState::UniswapV1(s)=&e.pool.state {
        use super::super::exec::uniswap_v1_wallet::IV1Wallet;
        return s.markets.iter().flatten().any(|m|m.address==log.address)
            && log.topics.len()==4 && [IV1Wallet::TokenPurchase::SIGNATURE_HASH, IV1Wallet::EthPurchase::SIGNATURE_HASH].contains(&log.topics[0]);
    }
    if let crate::core::types::PoolState::Rubicon(book)=&e.pool.state {
        use super::super::exec::rubicon_wallet::IBookWallet;
        return log.address==book.market && log.topics.first()==Some(&IBookWallet::emitTake::SIGNATURE_HASH) &&
            book.tokens.iter().enumerate().any(|(i,sell)| log.topics.get(2)==Some(&alloy_primitives::keccak256([sell.as_slice(),book.tokens[1-i].as_slice()].concat())));
    }
    if let crate::core::types::PoolState::Carbon(s)=&e.pool.state {return super::carbon_probe::emitted(s,&e.pool.reference,log);}
    if let crate::core::types::PoolState::MintClub(state)=&e.pool.state {
        return e.pool.reference==PoolRef::Contract(state.token) && super::super::pools::mint_club::emitted(state,log);
    }
    if super::super::quote::ekubo_v2_pricing::enabled(e) && let PoolRef::Singleton{manager,id}=e.pool.reference {
        return manager==log.address && log.topics.is_empty() && super::super::pools::abi_ekubo::decode_swap_log(&log.data).is_some_and(|r|r.0==id);
    }
    let swap = log
        .topics
        .first()
        .is_some_and(|t| swap_topics().contains(t));
    swap && match &e.pool.reference {
        PoolRef::Contract(a) => log.address == *a,
        PoolRef::Singleton { manager, id } => {
            log.address == *manager && log.topics.get(1) == Some(id)
        }
        // Пара общего контракта (PMM от оракула): событие выпускает общий контракт.
        PoolRef::Member { contract, .. } => log.address == *contract,
        // Решение 295: события пары многомонетного пула — у его контракта.
        PoolRef::Multi { contract, .. } => log.address == *contract,
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
        .filter(|(_, e)| {
            if let crate::core::types::PoolState::UniswapV1(s)=&e.pool.state {
                return s.markets.iter().flatten().any(|m|!logs.iter().any(|l|l.address==m.address && by_pool(e,l)));
            }
            !logs.iter().any(|l| by_pool(e, l)) && !super::super::pools::temple::defended(e,entries,logs)
        })
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
