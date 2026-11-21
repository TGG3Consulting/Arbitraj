//! Файл: venues/bsc_dex/pools/load_wombat_build.rs
//! Что делает: решение 484 — сборка состояния пары пула Wombat из разобранных ответов (без сети): PoolState::StableSwap
//! со StableData.wombat (cash / liability / decimals / цены активов, haircut, startCovRatio / endCovRatio, паузы
//! активов); общие поля StableData — для прочих частей бота: balances — cash в единицах монеты, rates — 10^(36 −
//! decimals) (у динамического пула × цена / 1e18 — оценка пробы quote/spot.rs), amp — ampFactor, fee — haircut / 10^8
//! (показ), halted — paused() пула. Вынесено из load_wombat_calls.rs (предел размера файла).
//! Кто вызывает: pools/load_wombat_calls.rs (реэкспорт build), pools/load_wombat.rs, tests/arbitrum_wombat_live.rs.
//! От чего зависит: load_wombat_calls.rs (Head, Side), math/stableswap_v1.rs (rate_for_decimals),
//! math/wombat_core.rs (from_wad, wad), core/types.
//! Настройки: нет.

use alloy_primitives::{Address, U256};

use crate::core::types::{PoolState, StableData, WombatData};

use super::super::math::stableswap_v1::rate_for_decimals;
use super::super::math::wombat_core::{from_wad, wad};
use super::load_wombat_calls::{Head, Side};

/// Сборка состояния пары. Принимает: Head, стороны, паузы активов по монетам, пару (token0, token1).
/// Возвращает: PoolState::StableSwap с частью Wombat или причину (монеты пары нет в пуле, decimals > 36, переполнение).
pub fn build(
    h: &Head,
    sides: &[Side],
    asset_paused: &[bool],
    pair: (Address, Address),
) -> Result<PoolState, String> {
    let index = |t: Address| {
        h.tokens
            .iter()
            .position(|c| *c == t)
            .map(|p| p as u8)
            .ok_or_else(|| format!("монеты {t} нет среди getTokens() пула"))
    };
    let (i0, i1) = (index(pair.0)?, index(pair.1)?);
    let dynamic = sides.iter().all(|s| s.price.is_some()) && !sides.is_empty();
    let (mut balances, mut rates) = (Vec::new(), Vec::new());
    for s in sides {
        balances.push(from_wad(s.cash, s.decimals).ok_or("cash не переводится в единицы монеты")?);
        let r = rate_for_decimals(s.decimals).ok_or("decimals > 36")?;
        rates.push(match s.price {
            Some(p) => r.checked_mul(p).ok_or("переполнение множителя")? / wad(),
            None => r,
        });
    }
    let w = WombatData {
        cash: sides.iter().map(|s| s.cash).collect(),
        liability: sides.iter().map(|s| s.liability).collect(),
        decimals: sides.iter().map(|s| s.decimals).collect(),
        price: if dynamic {
            sides.iter().filter_map(|s| s.price).collect()
        } else {
            Vec::new()
        },
        asset_paused: asset_paused.to_vec(),
        haircut: h.haircut,
        start_cov: h.start_cov,
        end_cov: h.end_cov,
    };
    Ok(PoolState::StableSwap(Box::new(StableData {
        balances,
        rates,
        amp: h.amp,
        fee: h.haircut / U256::from(100_000_000u64),
        amp_precision: U256::from(1u8),
        max_rounds: 0,
        strict: true,
        guard: None,
        halted: h.paused,
        i0,
        i1,
        ng: None,
        native: None,
        input_fee: false,
        pair_d: false,
        ema: false,
        wombat: Some(Box::new(w)),
    })))
}
