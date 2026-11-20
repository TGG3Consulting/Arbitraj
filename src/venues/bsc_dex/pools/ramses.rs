//! Файл: venues/bsc_dex/pools/ramses.rs
//! Что делает: решение 483 — события и создание пулов вариантов Ramses (pools/family_variant.rs) там, где они отличаются
//! от вида-основы: topic0 подписки, применение события к записи реестра, событие создания пула у фабрики.
//! - RamsesCl (v3_fork_ramses): Swap / Mint / Burn — Uniswap V3 (events_v3.rs); плюс FeeAdjustment(uint24 oldFee,
//!   uint24 newFee) пула — новая комиссия обмена прямо из события (ProtocolActions.sol L81-L84: states.fee = _fee;
//!   emit FeeAdjustment(_oldFee, _fee)). Создание — PoolCreated Uniswap V3 (как у v3_fork).
//! - RamsesClV2 (slipstream_ramses): Swap / Burn — Uniswap V3; Mint — свой (поле index; IRamsesV3PoolEvents.sol
//!   L22-L31, тики и ликвидность — те же поля, events_v3::modify); FeeAdjustment — как выше (ProtocolActions.sol
//!   L84-L86). Создание — PoolCreated(token0, token1, uint24 indexed fee, int24 tickSpacing, pool) (подпись Uniswap V3,
//!   IRamsesV3Factory.sol L13-L15), а не Slipstream.
//! - RamsesLegacy (solidly_v1_ramses): Sync(uint112, uint112) (IPair.sol L15, emit Pair.sol L171) — новые резервы;
//!   создание — PairCreated(token0, token1, pair, uint256) без stable (IPairFactory.sol L5, emit PairFactory.sol L189).
//!
//! Библиотека ProtocolActions — внешняя (функции external, вызов DELEGATECALL), поэтому FeeAdjustment приходит с
//! адресом пула (у старой линии — адрес RamsesBeaconProxy). Источники — шапка abi_ramses.rs.
//! Кто вызывает: pools/events.rs (topics, liquidity_topics, apply), pools/created_log.rs (created_topic, created),
//! exec/pool_cond_topics.rs (slot_topics), pools/load.rs, load_batch.rs, prefetch_plan.rs, load_v3_once.rs (is_cl).
//! От чего зависит: abi_ramses.rs, abi_v2v3.rs, events_v3.rs, events.rs (Applied), families.rs, family_variant.rs,
//! registry.rs, core/types (PoolState).
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256, U256};

use crate::core::types::PoolState;

use super::abi_ramses::{IRamsesPoolFee, IRamsesV3Pool};
use super::abi_v2v3::{IUniswapV3Pool, IV2Factory, IV2Pair, IV3Factory, IV3Pool};
use super::events::Applied;
use super::events_v3;
use super::families::Family;
use super::family_variant::Variant;
use super::registry::Entry;

/// Вариант Ramses с концентрированной ликвидностью (обе CL-линии). Ошибок нет.
pub fn is_cl(f: &Family) -> bool {
    matches!(f.variant, Variant::RamsesCl | Variant::RamsesClV2)
}

/// Любой вариант Ramses (события ведёт этот модуль). Ошибок нет.
pub fn handles(f: &Family) -> bool {
    is_cl(f) || f.variant == Variant::RamsesLegacy
}

/// topic0 событий, пишущих цену и ликвидность пула (без FeeAdjustment). None — не вариант Ramses. Ошибок нет.
pub fn slot_topics(f: &Family) -> Option<Vec<B256>> {
    let swap = IUniswapV3Pool::Swap::SIGNATURE_HASH;
    let burn = IV3Pool::Burn::SIGNATURE_HASH;
    Some(match f.variant {
        Variant::RamsesCl => vec![swap, IV3Pool::Mint::SIGNATURE_HASH, burn],
        Variant::RamsesClV2 => vec![swap, IRamsesV3Pool::Mint::SIGNATURE_HASH, burn],
        Variant::RamsesLegacy => vec![IV2Pair::Sync::SIGNATURE_HASH],
        _ => return None,
    })
}

/// topic0 подписки глубокого пула варианта: события цены и ликвидности + FeeAdjustment у CL-линий. Ошибок нет
/// (не вариант Ramses — пусто; вызывающий спрашивает только при handles).
pub fn topics(f: &Family) -> Vec<B256> {
    let mut t = slot_topics(f).unwrap_or_default();
    if is_cl(f) {
        t.push(IRamsesPoolFee::FeeAdjustment::SIGNATURE_HASH);
    }
    t
}

/// topic0 добавления ликвидности (мелкие пулы, pools/depth.rs): Mint своей линии; у легаси — Sync (как у Solidly,
/// events_solidly.rs liquidity_topics). Ошибок нет.
pub fn liquidity_topics(f: &Family) -> Vec<B256> {
    match f.variant {
        Variant::RamsesClV2 => vec![IRamsesV3Pool::Mint::SIGNATURE_HASH],
        Variant::RamsesLegacy => vec![IV2Pair::Sync::SIGNATURE_HASH],
        _ => vec![IV3Pool::Mint::SIGNATURE_HASH],
    }
}

/// Применяет событие пула варианта Ramses. Принимает: запись, лог. Возвращает: Applied. Ошибок нет: не по ABI или
/// состояние не того вида → Refresh (перечитать пул целиком).
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let Some(t0) = log.topic0().copied() else {
        return Applied::Ignored;
    };
    let d = &log.inner.data;
    if is_cl(e.family) && t0 == IRamsesPoolFee::FeeAdjustment::SIGNATURE_HASH {
        return match IRamsesPoolFee::FeeAdjustment::decode_log_data(d) {
            Ok(x) => {
                e.pool.fee_pips = Some(x.newFee.to::<u32>());
                Applied::Updated
            }
            Err(err) => Applied::Refresh(format!("Ramses FeeAdjustment не по ABI: {err}")),
        };
    }
    match e.family.variant {
        Variant::RamsesClV2 if t0 == IRamsesV3Pool::Mint::SIGNATURE_HASH => {
            match IRamsesV3Pool::Mint::decode_log_data(d) {
                Ok(m) => {
                    let delta = m.amount as i128;
                    events_v3::modify(e, m.tickLower.as_i32(), m.tickUpper.as_i32(), delta)
                }
                Err(err) => Applied::Refresh(format!("Ramses Mint не по ABI: {err}")),
            }
        }
        Variant::RamsesLegacy => sync(e, t0, log),
        _ => events_v3::apply(e, log),
    }
}

/// Sync(uint112, uint112) пары легаси: новые резервы (stable / decimals / комиссия сохраняются). Иное событие —
/// Ignored. Ошибок нет: не по ABI или состояние не Solidly → Refresh.
fn sync(e: &mut Entry, t0: B256, log: &Log) -> Applied {
    if t0 != IV2Pair::Sync::SIGNATURE_HASH {
        return Applied::Ignored;
    }
    let s = match IV2Pair::Sync::decode_log_data(&log.inner.data) {
        Ok(s) => s,
        Err(err) => return Applied::Refresh(format!("Ramses Sync не по ABI: {err}")),
    };
    match &mut e.pool.state {
        PoolState::Solidly {
            reserve0, reserve1, ..
        } => {
            *reserve0 = U256::from(s.reserve0);
            *reserve1 = U256::from(s.reserve1);
            Applied::Updated
        }
        _ => Applied::Refresh("Ramses Sync: состояние пула не Solidly — перечитать".into()),
    }
}

/// topic0 события создания пула у фабрики варианта, если он не тот, что у вида-основы. None — как у основы (RamsesCl
/// — PoolCreated Uniswap V3, как v3_fork) или не Ramses. Ошибок нет.
pub fn created_topic(f: &Family) -> Option<B256> {
    match f.variant {
        Variant::RamsesClV2 => Some(IV3Factory::PoolCreated::SIGNATURE_HASH),
        Variant::RamsesLegacy => Some(IV2Factory::PairCreated::SIGNATURE_HASH),
        _ => None,
    }
}

/// Пара и пул из события создания (только при created_topic = Some). Возвращает: (token0, token1, пул). Ошибки:
/// текст — лог не по ABI или не вариант со своим событием.
pub fn created(f: &Family, log: &Log) -> Result<(Address, Address, Address), String> {
    let d = &log.inner.data;
    match f.variant {
        Variant::RamsesClV2 => IV3Factory::PoolCreated::decode_log_data(d)
            .map(|x| (x.token0, x.token1, x.pool))
            .map_err(|e| format!("PoolCreated (Ramses CL V2) не по ABI: {e}")),
        Variant::RamsesLegacy => IV2Factory::PairCreated::decode_log_data(d)
            .map(|x| (x.token0, x.token1, x.pair))
            .map_err(|e| format!("PairCreated (Ramses Legacy) не по ABI: {e}")),
        _ => Err(format!("{}: своего события создания у вида нет", f.id)),
    }
}
