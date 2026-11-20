//! Файл: venues/bsc_dex/pools/events_ekubo.rs
//! Что делает: события пулов Ekubo (решение 447) — «topic0» для подписки и применение к записи пула. Все события — от
//! ядра Core (пул внутри ядра, PoolId — в данных, не в topics):
//! - анонимный лог обмена (log0, 116 байт: locker | PoolId | balanceUpdate | stateAfter; Core.sol L840-L847) — состояние
//!   после обмена целиком (цена, тик, ликвидность) → Updated. В подписке у него нет topic0: метка — B256::ZERO
//!   (ekubo_route.rs topic);
//! - PositionUpdated (ICore.sol L119-L126; Core.sol L358-L450) — stateAfter, у концентрированного пула ещё тики границ
//!   позиции: нижний — liquidityDelta += Δ, верхний — −= Δ, liquidityNet += Δ у обоих (Core.sol L285-L319, _updateTick),
//!   бит битмапа переключается при переходе liquidityNet 0 ↔ не 0 (L302-L303); тики вне прочитанных слов не трогаются
//!   (карта там не читалась, расчёт туда не идёт). liquidityNet ушёл бы ниже нуля — карта разошлась → Refresh;
//! - PositionFeesCollected — состояние обмена не меняет, но у пула с MEVCapture перед сбором расширение обновляет своё
//!   состояние (beforeCollectFees → accumulatePoolFees, MEVCapture.sol L89-L95) → Updated только у такого пула;
//! - PoolInitialized, FeesAccumulated — состояние обмена не меняют → Ignored.
//!
//! MEVCapture (MEVCapture.sol L183-L206; beforeUpdatePosition — L97-L103 → accumulatePoolFees L105-L125): в новой секунде
//! расширение записывает (время блока, тик до действия). Время блока берётся из лога (blockTimestamp); его нет — пул
//! перечитывается (Refresh), не гадаем.
//! Кто вызывает: pools/events.rs.
//! От чего зависит: abi_ekubo.rs, math/ekubo_bitmap.rs, math/ekubo_config.rs, events.rs, registry.rs.
//! Настройки: нет.

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{B256, U256};

use crate::core::types::{EkuboData, EkuboTick, PoolRef, PoolState};

use super::super::math::ekubo_bitmap::word_index;
use super::super::math::ekubo_config::{self, Shape};
use super::abi_ekubo::{IEkuboCore as C, decode_state, decode_swap_log, position_bounds};
use super::events::Applied;
use super::registry::Entry;

/// Метка анонимного лога обмена в списке «topic0» (у log0 topic0 нет). Ошибок нет.
pub const SWAP_MARK: B256 = B256::ZERO;

/// «topic0» событий, меняющих состояние обмена: анонимный обмен (SWAP_MARK) и PositionUpdated. Ошибок нет.
pub fn topics() -> Vec<B256> {
    vec![SWAP_MARK, C::PositionUpdated::SIGNATURE_HASH]
}

/// topic0 событий ликвидности (PositionUpdated). Ошибок нет.
pub fn liquidity_topics() -> Vec<B256> {
    vec![C::PositionUpdated::SIGNATURE_HASH]
}

/// Применяет событие (см. шапку). Ошибок нет: не разобрано или не сходится → Refresh.
pub fn apply(e: &mut Entry, log: &Log) -> Applied {
    let PoolRef::Singleton { id, .. } = e.pool.reference else {
        return Applied::Ignored;
    };
    let title = e.family.title;
    let ts = log.block_timestamp;
    let PoolState::Ekubo(d) = &mut e.pool.state else {
        return Applied::Refresh(format!("{title}: у пула Ekubo нет состояния"));
    };
    let refresh = |w: &str| Applied::Refresh(format!("{title} {id}: {w}"));
    let data = &log.inner.data.data[..];
    if log.topics().is_empty() {
        let Some((pool, _, state)) = decode_swap_log(data) else {
            return Applied::Ignored;
        };
        if pool != id {
            return Applied::Ignored;
        }
        if !touch_mev(d, ts) {
            return refresh("нет времени блока в логе — состояние MEVCapture перечитать");
        }
        set_state(d, &state);
        return Applied::Updated;
    }
    if log.topic0() == Some(&C::PositionFeesCollected::SIGNATURE_HASH) {
        let Ok(ev) = C::PositionFeesCollected::decode_log_data(&log.inner.data) else {
            return refresh("PositionFeesCollected не по ABI");
        };
        if ev.poolId != id || d.mev.is_none() {
            return Applied::Ignored;
        }
        if !touch_mev(d, ts) {
            return refresh("нет времени блока в логе — состояние MEVCapture перечитать");
        }
        return Applied::Updated;
    }
    if log.topic0() != Some(&C::PositionUpdated::SIGNATURE_HASH) {
        return Applied::Ignored;
    }
    let Ok(ev) = C::PositionUpdated::decode_log_data(&log.inner.data) else {
        return refresh("PositionUpdated не по ABI");
    };
    if ev.poolId != id {
        return Applied::Ignored;
    }
    if !touch_mev(d, ts) {
        return refresh("нет времени блока в логе — состояние MEVCapture перечитать");
    }
    if let Shape::Concentrated { spacing } = ekubo_config::shape(&d.config) {
        let (lower, upper) = position_bounds(&ev.positionId);
        let delta = ev.liquidityDelta;
        let ok = update_tick(d, spacing, lower, delta, false)
            && update_tick(d, spacing, upper, delta, true);
        if !ok {
            return refresh("PositionUpdated не сходится с картой тиков — перечитать пул");
        }
    }
    set_state(d, &ev.stateAfter);
    Applied::Updated
}

/// Цена, тик и ликвидность из stateAfter. Ошибок нет.
fn set_state(d: &mut EkuboData, w: &B256) {
    let (sqrt, tick, liquidity) = decode_state(w);
    d.sqrt_ratio = sqrt;
    d.tick = tick;
    d.liquidity = liquidity;
}

/// MEVCapture: действие в новой секунде записывает (время блока, тик до действия). Принимает: состояние, время блока
/// лога. Возвращает: false — у пула MEVCapture, а времени в логе нет. Ошибок нет.
fn touch_mev(d: &mut EkuboData, ts: Option<u64>) -> bool {
    let Some(m) = d.mev.as_mut() else {
        return true;
    };
    let Some(ts) = ts else {
        return false;
    };
    if u64::from(m.last_update_time) != ts {
        m.last_update_time = ts as u32;
        m.tick_last = d.tick;
    }
    d.block_ts = d.block_ts.max(ts);
    true
}

/// Тик границы позиции (как _updateTick): delta ± Δ, net + Δ, бит битмапа при переходе net 0 ↔ не 0. Вне прочитанных
/// слов — не трогается. Возвращает: false — net ушёл бы ниже нуля или переполнение (карта разошлась). Ошибок нет.
fn update_tick(d: &mut EkuboData, spacing: u32, tick: i32, delta: i128, upper: bool) -> bool {
    let (word, bit) = word_index(tick, spacing);
    if !(d.words.0..=d.words.1).contains(&word) {
        return true;
    }
    let pos = d.ticks.binary_search_by_key(&tick, |t| t.tick);
    let zero = EkuboTick {
        tick,
        net: 0,
        delta: 0,
    };
    let cur = pos.map_or(zero, |i| d.ticks[i]);
    let Some(net) = cur.net.checked_add_signed(delta) else {
        return false;
    };
    let next_delta = if upper {
        cur.delta.checked_sub(delta)
    } else {
        cur.delta.checked_add(delta)
    };
    let Some(next_delta) = next_delta else {
        return false;
    };
    if (cur.net == 0) != (net == 0) {
        flip(d, word, bit);
    }
    let t = EkuboTick {
        tick,
        net,
        delta: next_delta,
    };
    match (pos, net == 0) {
        (Ok(i), true) => {
            d.ticks.remove(i);
        }
        (Ok(i), false) => d.ticks[i] = t,
        (Err(i), false) => d.ticks.insert(i, t),
        (Err(_), true) => {}
    }
    true
}

/// Переключает бит слова битмапа (flipTick, tickBitmap.sol L200-L204). Нулевое слово не хранится. Ошибок нет.
fn flip(d: &mut EkuboData, word: u64, bit: u32) {
    let mask = U256::from(1u8) << bit as usize;
    match d.bitmap.binary_search_by_key(&word, |(w, _)| *w) {
        Ok(i) => {
            d.bitmap[i].1 ^= mask;
            if d.bitmap[i].1.is_zero() {
                d.bitmap.remove(i);
            }
        }
        Err(i) => d.bitmap.insert(i, (word, mask)),
    }
}
