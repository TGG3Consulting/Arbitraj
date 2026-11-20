//! Файл: venues/bsc_dex/math/ekubo_bitmap.rs
//! Что делает: битмап тиков Ekubo (решение 447) — слово и бит тика, тик по слову и биту, поиск следующего / предыдущего
//! инициализированного тика ровно как ядро при skipAhead = 0 (шаг EKUBO исполнителя и квотер сверки зовут обмен с
//! skipAhead = 0): не найден в слове — граница слова (не инициализирован), обмен останавливается на ней. Слова — только
//! прочитанные (EkuboData.words): нужное слово не прочитано — None (расчёт дальше не идёт).
//! Кто вызывает: math/ekubo_swap.rs, pools/load_ekubo.rs (какие слова читать), pools/events_ekubo.rs (переключение бита).
//! От чего зависит: alloy-primitives, core/types (EkuboData), math/ekubo_tick.rs.
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa: src/math/tickBitmap.sol L174 (TICK_BITMAP_STORAGE_OFFSET), L178-L184
//! (tickToBitmapWordAndIndex: floor(tick / spacing) + смещение; слово — >> 8, бит — & 0xff), L188-L193
//! (bitmapWordAndIndexToTick), L206-L244 (findNextInitializedTick), L246-L282 (findPrevInitializedTick);
//! src/types/bitmap.sol L333-L340 (leSetBit — старший бит ≤ index), L350-L355 (geSetBit — младший бит ≥ index).

use alloy_primitives::U256;

use crate::core::types::EkuboData;

use super::ekubo_tick::{MAX_TICK, MIN_TICK};

/// TICK_BITMAP_STORAGE_OFFSET (tickBitmap.sol L174).
pub const OFFSET: i64 = 89_421_695;

/// Слово и бит тика. Принимает: тик, шаг (> 0). Возвращает: (слово, бит). Ошибок нет.
pub fn word_index(tick: i32, spacing: u32) -> (u64, u32) {
    let raw = i64::from(tick).div_euclid(i64::from(spacing)) + OFFSET;
    ((raw >> 8) as u64, (raw & 0xff) as u32)
}

/// Тик по слову и биту (i64: граница крайнего слова может выйти за MAX_TICK — её обрезает вызывающий). Ошибок нет.
pub fn tick_of(word: u64, index: u32, spacing: u32) -> i64 {
    let raw = ((word as i64) << 8) + i64::from(index);
    (raw - OFFSET) * i64::from(spacing)
}

/// Биты слова из прочитанного: Some(биты) — слово прочитано (нулевое — не хранится), None — не прочитано.
fn bits(d: &EkuboData, word: u64) -> Option<U256> {
    if word < d.words.0 || word > d.words.1 {
        return None;
    }
    Some(
        d.bitmap
            .iter()
            .find(|(w, _)| *w == word)
            .map_or(U256::ZERO, |(_, b)| *b),
    )
}

/// findNextInitializedTick при skipAhead = 0. Принимает: состояние, тик, шаг. Возвращает: (тик, инициализирован).
/// None — нужное слово не прочитано.
pub fn next(d: &EkuboData, from: i32, spacing: u32) -> Option<(i32, bool)> {
    let (word, index) = word_index(from.checked_add(spacing as i32)?, spacing);
    let b = bits(d, word)?;
    // geSetBit: младший установленный бит ≥ index.
    let masked = b & !((U256::from(1u8) << index as usize) - U256::from(1u8));
    if !masked.is_zero() {
        let bit = masked.trailing_zeros() as u32;
        return Some((tick_of(word, bit, spacing) as i32, true));
    }
    let t = tick_of(word, 255, spacing).min(i64::from(MAX_TICK));
    Some((t as i32, false))
}

/// findPrevInitializedTick при skipAhead = 0. Принимает: состояние, тик, шаг. Возвращает: (тик, инициализирован).
/// None — нужное слово не прочитано.
pub fn prev(d: &EkuboData, from: i32, spacing: u32) -> Option<(i32, bool)> {
    let (word, index) = word_index(from, spacing);
    let b = bits(d, word)?;
    // leSetBit: старший установленный бит ≤ index (маска 2^(index+1) − 1; при 255 — все биты).
    let mask = if index >= 255 {
        U256::MAX
    } else {
        (U256::from(1u8) << (index as usize + 1)) - U256::from(1u8)
    };
    let masked = b & mask;
    if !masked.is_zero() {
        let bit = 255 - masked.leading_zeros() as u32;
        return Some((tick_of(word, bit, spacing) as i32, true));
    }
    let t = tick_of(word, 0, spacing).max(i64::from(MIN_TICK));
    Some((t as i32, false))
}

/// liquidityDelta инициализированного тика (0 — тика нет в карте). Ошибок нет.
pub fn delta_at(d: &EkuboData, tick: i32) -> i128 {
    d.ticks
        .binary_search_by_key(&tick, |t| t.tick)
        .map_or(0, |i| d.ticks[i].delta)
}
