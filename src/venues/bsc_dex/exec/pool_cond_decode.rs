//! Файл: venues/bsc_dex/exec/pool_cond_decode.rs
//! Что делает: ревью 438 (C) — разбор прочитанных слов слотов условия пулов по раскладке контракта и сверка с
//! состоянием того же пула на том же блоке (quote/cycle_legs.rs fresh_at): V2 — reserve0/1; Uniswap V3, PancakeSwap V3,
//! Slipstream, Uniswap V4 — sqrtPriceX96, tick, liquidity; Kyber Elastic — sqrtP, currentTick, baseL; LB 2.1 —
//! activeId; LB 2.0 (решение 445) — activeId слота 6 [0,24). Что делать при расхождении — exec/pool_cond_check.rs.
//!
//! Раскладка в слове (правило Solidity: первое поле слота — в младших битах, docs.soliditylang.org «Layout of State
//! Variables in Storage»): V2 слот 8 — reserve0 [0,112), reserve1 [112,224) (UniswapV2Pair.sol:22-24); V3-подобные
//! slot0 — sqrtPriceX96 [0,160), tick [160,184) (UniswapV3Pool.sol:56-74, PancakeV3Pool.sol:60-78, slipstream
//! 193a6b85 CLPool.sol:53-66), liquidity — младшие 128 бит своего слота (slipstream CLPool.sol:105-107: liquidity,
//! затем maxLiquidityPerTick); Kyber слот 3 — sqrtP [0,160), nearestCurrentTick [160,184), currentTick [184,208),
//! слот 4 — baseL [0,128) (ks-elastic-sc 4ab08c0a PoolStorage.sol:21-32); LB _parameters — activeId [232,256)
//! (joe-v2 8eeb42d7 src/libraries/PairParameterHelper.sol:26, 46); V4 slot0 — sqrtPriceX96 [0,160), tick [160,184)
//! (v4-core bea7f6d1 src/types/Slot0.sol:9, 33-38), liquidity S+3 — младшие 128 бит (StateLibrary.sol:19).
//! Кто вызывает: exec/pool_cond_check.rs.
//! От чего зависит: pools/families.rs (Proto), core/types (PoolState).
//! Настройки: нет.

use alloy_primitives::{Address, B256, U256};
use serde_json::{Map, Value};

use crate::core::types::PoolState;

use super::super::pools::families::Proto;

/// Поле: имя, значение из слова слота, значение из состояния пула.
type Field = (&'static str, U256, U256);

/// Вид пула и состояние не сочетаются.
const MISMATCH: &str = "вид пула и его состояние не сочетаются — самопроверка невозможна";

/// Биты [off, off + len) слова. Ошибок нет.
fn bits(w: U256, off: usize, len: usize) -> U256 {
    (w >> off) & (U256::MAX >> (256 - len))
}

/// int24 тика как 24 бита дополнительного кода (как в слове). Ошибок нет.
fn tick24(t: i32) -> U256 {
    U256::from((t as u32) & 0x00FF_FFFF)
}

/// Слова слотов адреса из knownAccounts по порядку правила. None — не все прочитаны. Ошибок нет.
pub fn words(a: Address, slots: &[B256], known: &Map<String, Value>) -> Option<Vec<U256>> {
    let vals = known.get(&format!("{a:#x}"))?.as_object()?;
    slots
        .iter()
        .map(|s| {
            let v = vals.get(&format!("{s:#x}"))?.as_str()?;
            v.parse::<B256>().ok().map(|b| U256::from_be_bytes(b.0))
        })
        .collect()
}

/// Поля V3-подобного пула: (slot0, liquidity) слова и (sqrtPriceX96, tick, liquidity) состояния. Ошибок нет.
fn v3(w: (U256, U256), s: U256, t: i32, l: u128) -> Vec<Field> {
    vec![
        ("sqrtPriceX96", bits(w.0, 0, 160), s),
        ("tick", bits(w.0, 160, 24), tick24(t)),
        ("liquidity", bits(w.1, 0, 128), U256::from(l)),
    ]
}

/// Вид пула сочетается с видом состояния. Ошибок нет.
fn fits(p: Proto, st: &PoolState) -> bool {
    match st {
        PoolState::V2 { .. } => p == Proto::V2,
        PoolState::V3 { .. } => {
            matches!(p, Proto::UniswapV3 | Proto::PancakeV3 | Proto::Slipstream)
        }
        PoolState::Cl { .. } => p == Proto::UniswapV4,
        PoolState::Elastic { .. } => p == Proto::KyberElastic,
        PoolState::LbBin(d) => p == if d.v20 { Proto::LbV20 } else { Proto::LbV21 },
        _ => false,
    }
}

/// Поля пула: из слов и из состояния (раскладка — шапка). Ошибки: текст — вид и состояние не сочетаются.
fn fields(p: Proto, st: &PoolState, w: &[U256]) -> Result<Vec<Field>, String> {
    if !fits(p, st) {
        return Err(MISMATCH.into());
    }
    let w0 = w.first().copied().unwrap_or_default();
    let w1 = w.get(1).copied().unwrap_or_default();
    Ok(match st {
        PoolState::V2 { reserve0, reserve1 } => vec![
            ("reserve0", bits(w0, 0, 112), *reserve0),
            ("reserve1", bits(w0, 112, 112), *reserve1),
        ],
        PoolState::V3 {
            sqrt_price_x96,
            liquidity,
            tick,
        }
        | PoolState::Cl {
            sqrt_price_x96,
            liquidity,
            tick,
            ..
        } => v3((w0, w1), *sqrt_price_x96, *tick, *liquidity),
        PoolState::Elastic {
            sqrt_price_x96,
            tick,
            base_l,
            ..
        } => vec![
            ("sqrtP", bits(w0, 0, 160), *sqrt_price_x96),
            ("currentTick", bits(w0, 184, 24), tick24(*tick)),
            ("baseL", bits(w1, 0, 128), U256::from(*base_l)),
        ],
        // Решение 445: LB v2.0 — activeId в младших 24 битах слота 6 (LBPair.sol v2.0.0 L1039).
        PoolState::LbBin(d) if d.v20 => {
            vec![("activeId", bits(w0, 0, 24), U256::from(d.active_id))]
        }
        PoolState::LbBin(d) => vec![("activeId", bits(w0, 232, 24), U256::from(d.active_id))],
        _ => return Err(MISMATCH.into()),
    })
}

/// Расхождение одного пула. Принимает: вид, состояние пула на блоке плана, слова слотов (None — не найдены).
/// Возвращает: None — слова сошлись с состоянием; иначе причина. Ошибок нет.
pub fn differ(p: Proto, st: &PoolState, w: Option<Vec<U256>>) -> Option<String> {
    let Some(w) = w else {
        return Some("слово слота не найдено среди прочитанных".into());
    };
    match fields(p, st, &w) {
        Err(why) => Some(why),
        Ok(pairs) => {
            let bad: Vec<String> = pairs
                .iter()
                .filter(|(_, a, b)| a != b)
                .map(|(n, a, b)| format!("{n}: в слоте {a}, у пула {b}"))
                .collect();
            (!bad.is_empty()).then(|| bad.join(", "))
        }
    }
}
