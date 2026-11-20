//! Файл: venues/bsc_dex/exec/pool_cond_topics.rs
//! Что делает: ревью 438 (D) — topic0 событий пула маршрута, которые пишут слоты условия пулов (exec/pool_cond.rs
//! How::Slots): по ним разбор проигрыша (exec/tip_judge.rs) считает перехватчиком только транзакцию, изменившую эти
//! слоты, а не любую с логом на адресе пула. Пул с корнем хранилища или без условия — фильтра нет (None: корень ловит
//! любую запись в хранилище пула, событие любое).
//! Какие события пишут слоты (исходники — таблица в шапке exec/pool_cond.rs):
//! - V2, слот 8 (резервы): каждая запись — через _update, он шлёт Sync (Uniswap/v2-core 6a9e7c97
//!   contracts/UniswapV2Pair.sol:73-82); Swap и Mint — в той же транзакции;
//! - Uniswap V3, PancakeSwap V3, Slipstream (slot0, liquidity): Swap пишет slot0 и liquidity, Mint/Burn в текущем
//!   диапазоне — liquidity (v3-core 5ea68a77 UniswapV3Pool.sol:336-351); события — как слушает pools/events_v3.rs;
//! - Kyber Elastic (poolData, baseL/reinvestL): Swap, Mint, Burn, BurnRTokens (pools/abi_elastic.rs);
//! - LB 2.1 (_parameters, _reserves): Swap, DepositedToBins, WithdrawnFromBins (lfj-gg/joe-v2 8eeb42d7
//!   src/LBPair.sol:546-550, 608, 652, 712; pools/abi_lb.rs);
//! - Uniswap V4 (slot0 и liquidity пула у PoolManager): Swap и ModifyLiquidity с topics[1] = PoolId (pools/abi_v4.rs).
//!
//! Не ловятся (сделка не теряет долю чаевых из-за них): записи slot0 без этих событий (V3 setFeeProtocol,
//! increaseObservationCardinalityNext) — это не конкурент-арбитражник. topic0 — SIGNATURE_HASH сигнатур из ABI папки
//! pools (alloy::sol!, сигнатуры — из официальных исходников, ссылки в шапках abi_*.rs).
//! Кто вызывает: exec/tip_cycle.rs (choose — план разбора).
//! От чего зависит: exec/pool_cond.rs (rule), pools/{abi_v2v3, abi_v4, abi_elastic, abi_lb, events_v3, families}.rs.
//! Настройки: нет.

use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

use super::super::pools::abi_elastic::IElasticPool;
use super::super::pools::abi_lb::ILBPair;
use super::super::pools::abi_lb20::ILBPairV20;
use super::super::pools::abi_v2v3::IV2Pair;
use super::super::pools::abi_v4::IUniswapV4;
use super::super::pools::events_v3;
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;
use super::pool_cond::{self, How};

/// topic0 событий, пишущих слоты условия пула. Принимает: запись реестра. Возвращает: список; None — фильтра нет
/// (корень хранилища или без условия). Ошибок нет.
pub fn topics(e: &Entry) -> Option<Vec<B256>> {
    if !matches!(pool_cond::rule(e), How::Slots(..)) {
        return None;
    }
    let p = e.family.proto;
    Some(match p {
        Proto::V2 => vec![
            IV2Pair::Sync::SIGNATURE_HASH,
            IV2Pair::Swap::SIGNATURE_HASH,
            IV2Pair::Mint::SIGNATURE_HASH,
        ],
        Proto::UniswapV3 | Proto::PancakeV3 | Proto::Slipstream => events_v3::topics(p),
        Proto::KyberElastic => vec![
            IElasticPool::Swap::SIGNATURE_HASH,
            IElasticPool::Mint::SIGNATURE_HASH,
            IElasticPool::Burn::SIGNATURE_HASH,
            IElasticPool::BurnRTokens::SIGNATURE_HASH,
        ],
        Proto::LbV21 => vec![
            ILBPair::Swap::SIGNATURE_HASH,
            ILBPair::DepositedToBins::SIGNATURE_HASH,
            ILBPair::WithdrawnFromBins::SIGNATURE_HASH,
        ],
        // Решение 445: LB v2.0 — события на бин (abi_lb20.rs).
        Proto::LbV20 => vec![
            ILBPairV20::Swap::SIGNATURE_HASH,
            ILBPairV20::DepositedToBin::SIGNATURE_HASH,
            ILBPairV20::WithdrawnFromBin::SIGNATURE_HASH,
        ],
        Proto::UniswapV4 => vec![
            IUniswapV4::Swap::SIGNATURE_HASH,
            IUniswapV4::ModifyLiquidity::SIGNATURE_HASH,
        ],
        _ => return None,
    })
}
