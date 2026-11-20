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
//! - LB 2.1 (_parameters, _reserves) — все записи этих слотов в lfj-gg/joe-v2 v2.1.0 8eeb42d7 src/LBPair.sol (ревью
//!   439 (3)): Swap (546, 550), DepositedToBins (652; запись _parameters 1006 — в той же транзакции, рядом
//!   CompositionFees), WithdrawnFromBins (712), FlashLoan (608, _reserves), CollectedProtocolFees (740, _reserves),
//!   OracleLengthIncreased (760, _parameters), ForcedDecay (807, _parameters), StaticFeeParametersSet (911,
//!   _parameters); строка 78 — конструктор. Сигнатуры — src/interfaces/ILBPair.sol:37-78 (pools/abi_lb.rs;
//!   OracleLengthIncreased в abi_lb.rs нет — topic0 = keccak256 сигнатуры здесь);
//! - Uniswap V4 (slot0 и liquidity пула у PoolManager): Swap и ModifyLiquidity с topics[1] = PoolId (pools/abi_v4.rs).
//!
//! Ревью 439 (4): фильтр отправки строится по ИТОГОВОМУ условию (by_cond — knownAccounts после самопроверки
//! exec/pool_cond_check.rs): слоты пула заменены корнем, сняты (пул без условия) или не прочитаны — None, любое
//! событие пула (корень меняет любая запись; раскладка не подтверждена — список событий по виду DEX не доказан).
//! Условия пулов у отправки нет (send.pool_cond_mode = off или не задана, нет окна у канала sequencer, или ни один пул
//! не попал в условие) — фильтр по виду DEX (topics): перехватчик отката — только транзакция с событием, пишущим
//! слоты цены/ликвидности пула (таблица выше); пулы с корнем или без условия по правилу — любое событие (None).
//!
//! Не ловятся (сделка не теряет долю чаевых из-за них): записи slot0 без этих событий (V3 setFeeProtocol,
//! increaseObservationCardinalityNext) — это не конкурент-арбитражник. topic0 — SIGNATURE_HASH сигнатур из ABI папки
//! pools (alloy::sol!, сигнатуры — из официальных исходников, ссылки в шапках abi_*.rs).
//! Кто вызывает: exec/tip_cycle.rs (choose — план разбора, by_cond).
//! От чего зависит: exec/pool_cond.rs (rule), pools/{abi_v2v3, abi_v4, abi_elastic, abi_lb, events_v3, families}.rs.
//! Настройки: нет.

use alloy::sol_types::SolEvent;
use alloy_primitives::{B256, keccak256};
use serde_json::{Map, Value};

use super::super::pools::abi_elastic::IElasticPool;
use super::super::pools::abi_lb::ILBPair;
use super::super::pools::abi_lb20::ILBPairV20;
use super::super::pools::abi_v2v3::IV2Pair;
use super::super::pools::abi_v4::IUniswapV4;
use super::super::pools::events_v3;
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;
use super::pool_cond::{self, How};

/// Ревью 439 (4): topic0 по итоговому условию отправки (см. шапку). Принимает: запись реестра, knownAccounts отправки
/// (None — условия пулов у отправки нет: фильтр по виду DEX, topics). Возвращает: как topics, если все слоты правила
/// пула остались в условии; иначе None — любое событие пула. Ошибок нет.
pub fn by_cond(e: &Entry, known: Option<&Map<String, Value>>) -> Option<Vec<B256>> {
    let Some(known) = known else {
        return topics(e);
    };
    let How::Slots(a, slots, _) = pool_cond::rule(e) else {
        return None;
    };
    let kept = match known.get(&format!("{a:#x}")) {
        Some(Value::Object(m)) => slots.iter().all(|s| m.contains_key(&format!("{s:#x}"))),
        _ => false,
    };
    if kept { topics(e) } else { None }
}

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
            ILBPair::FlashLoan::SIGNATURE_HASH,
            ILBPair::CollectedProtocolFees::SIGNATURE_HASH,
            ILBPair::ForcedDecay::SIGNATURE_HASH,
            ILBPair::StaticFeeParametersSet::SIGNATURE_HASH,
            // joe-v2 v2.1.0 8eeb42d7 src/interfaces/ILBPair.sol:76.
            keccak256("OracleLengthIncreased(address,uint16)"),
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
