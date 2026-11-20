//! Файл: venues/bsc_dex/pools/events_ekubo_ticks.rs
//! Что делает: карта тиков пула Ekubo по событию PositionUpdated (решение 447) — тик границы позиции: liquidityDelta
//! (нижний += Δ, верхний −= Δ), liquidityNet += Δ, бит битмапа при переходе liquidityNet 0 ↔ не 0. Тики вне
//! прочитанных слов не трогаются. Вынесено из events_ekubo.rs.
//! Кто вызывает: pools/events_ekubo.rs.
//! От чего зависит: core/types (EkuboData, EkuboTick), math/ekubo_bitmap.rs (word_index).
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/Core.sol L285-L319 (_updateTick), L302-L303 (переключение бита);
//! src/math/tickBitmap.sol L36-L40 (flipTick).

use alloy_primitives::U256;

use crate::core::types::{EkuboData, EkuboTick};

use super::super::math::ekubo_bitmap::word_index;

/// Тик границы позиции (как _updateTick): delta ± Δ, net + Δ, бит битмапа при переходе net 0 ↔ не 0. Вне прочитанных
/// слов — не трогается. Возвращает: false — net ушёл бы ниже нуля или переполнение (карта разошлась). Ошибок нет.
pub fn update_tick(d: &mut EkuboData, spacing: u32, tick: i32, delta: i128, upper: bool) -> bool {
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

/// Переключает бит слова битмапа (flipTick, tickBitmap.sol L36-L40). Нулевое слово не хранится. Ошибок нет.
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
