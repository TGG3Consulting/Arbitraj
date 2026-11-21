//! Файл: venues/bsc_dex/math/ekubo_time.rs
//! Что делает: времена ордеров TWAMM Ekubo (решение 454) — точный порт: шаг допустимого времени (растёт степенями 16 с
//! удалением от текущего), следующее допустимое время, слово и бит времени в битмапе времён и поиск следующего
//! инициализированного времени по прочитанным словам битмапа. Слово, от которого зависит результат, не прочитано —
//! None (расчёт не идёт за край прочитанного); слово первого допустимого времени за пределом поиска не нужно (search).
//! Кто вызывает: math/ekubo_virtual.rs, pools/load_ekubo_twamm.rs (какие слова читать).
//! От чего зависит: alloy-primitives, core/types (EkuboTwamm).
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/math/time.sol L17-L31 (computeStepSize), L44-L65
//! (nextValidTime); src/math/timeBitmap.sol L10-L15 (timeToBitmapWordAndIndex), L19-L23 (обратно), L34-L54
//! (findNextInitializedTime, geSetBit — types/bitmap.sol L68-L73), L60-L83 (searchForNextInitializedTime).

use alloy_primitives::U256;

use crate::core::types::EkuboTwamm;

/// computeStepSize: 256, а дальше 4095 с от текущего — 2^(старший бит разницы, вниз до кратного 4). Ошибок нет.
pub fn step_size(current: u64, time: u64) -> u64 {
    if u128::from(time) > u128::from(current) + 4095 {
        let diff = time - current;
        let msb = 63 - diff.leading_zeros();
        1u64 << (msb - msb % 4)
    } else {
        256
    }
}

/// nextValidTime: следующее допустимое время после `after`; 0 — нет (дальше 2^32 − 1 от текущего). Ошибок нет.
pub fn next_valid_time(current: u64, after: u64) -> u64 {
    let up = |step: u64| {
        let t = u128::from(after) + u128::from(step);
        t - t % u128::from(step)
    };
    let step = step_size(current, after);
    let mut next = up(step);
    if next != 0 {
        let n = u64::try_from(next).unwrap_or(u64::MAX);
        let second = step_size(current, n);
        if second != step {
            next = up(second);
        }
    }
    if next > u128::from(current) + u128::from(u32::MAX) {
        0
    } else {
        next as u64
    }
}

/// (слово, бит) времени: время >> 16, (время >> 8) & 0xff. Ошибок нет.
pub fn word_index(time: u64) -> (u64, u32) {
    (time >> 16, ((time >> 8) & 0xff) as u32)
}

/// Слово битмапа времён из прочитанного. None — слово не прочитано. Ошибок нет.
fn word(t: &EkuboTwamm, w: u64) -> Option<U256> {
    if t.words.binary_search(&w).is_err() {
        return None;
    }
    let bits = t
        .bitmap
        .iter()
        .find(|(x, _)| *x == w)
        .map_or(U256::ZERO, |(_, b)| *b);
    Some(bits)
}

/// findNextInitializedTime: (время, инициализировано). None — слово не прочитано. Ошибок нет.
pub fn find_next(t: &EkuboTwamm, from: u64) -> Option<(u64, bool)> {
    let (w, idx) = word_index(from);
    let bits = word(t, w)?;
    let masked = bits & !((U256::from(1u8) << idx as usize) - U256::from(1u8));
    let (index, init) = if masked.is_zero() {
        (255u64, false)
    } else {
        (u64::from(masked.trailing_zeros() as u32), true)
    };
    Some(((w << 16) + (index << 8), init))
}

/// searchForNextInitializedTime: наименьшее инициализированное время в (from, until] или until. Принимает: состояние,
/// реальное время последнего исполнения (база допустимых времён), from, until. None — нужное слово не прочитано.
///
/// Допустимое время за `until`: контракт читает его слово (timeBitmap.sol L76), но результат от слова не зависит —
/// findNextInitializedTime возвращает время ≥ своего fromTime (допустимые времена кратны 256: шаг computeStepSize —
/// 256 или степень 16 ≥ 4096, time.sol L17-L31; бит ищется от индекса fromTime вверх, а без бита — индекс 255,
/// timeBitmap.sol L40-L52), и L77-L80 сводят его к (until, false). Поэтому здесь — сразу (until, false) без слова:
/// край горизонта dex.ekubo_twamm_horizon_s точен, хотя слово первого допустимого времени за ним не прочитано.
pub fn search(t: &EkuboTwamm, last: u64, from: u64, until: u64) -> Option<(u64, bool)> {
    let (mut next, mut init) = (from, false);
    while !init && next != until {
        let valid = next_valid_time(last, next);
        if valid == 0 || valid > until {
            return Some((until, false));
        }
        (next, init) = find_next(t, valid)?;
        if next > until {
            (next, init) = (until, false);
        }
    }
    Some((next, init))
}

/// Слова битмапа, от которых зависит поиск до любого предела ≤ `end`: слова всех допустимых времён (от `last`) в
/// (last, end] (первое допустимое время за пределом поиска слова не требует — search). Ошибок нет.
pub fn words_until(last: u64, end: u64) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    let mut t = last;
    loop {
        t = next_valid_time(last, t);
        if t == 0 || t > end {
            break;
        }
        let w = t >> 16;
        if out.last() != Some(&w) {
            out.push(w);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{next_valid_time, step_size};

    /// Векторы официального теста (в пределах u64): evm-contracts@e5574ab test/math/time.t.sol L50-L70, L152-L159
    /// (time.sol не менялся с 61d09daa — git diff пуст).
    #[test]
    fn official_vectors() {
        assert_eq!(step_size(0, 4), 256);
        assert_eq!(step_size(4, 0), 256);
        assert_eq!(step_size(7553, 7936), 256);
        assert_eq!(step_size(7553, 8192), 256);
        assert_eq!(step_size(4_026_531_839, 4_294_967_295), 1 << 28);
        assert_eq!(next_valid_time(0, 15), 256);
        assert_eq!(next_valid_time(0, 16), 256);
        assert_eq!(next_valid_time(0, 255), 256);
        assert_eq!(next_valid_time(0, 256), 512);
        assert_eq!(next_valid_time(1, 300), 512);
        assert_eq!(next_valid_time(7679, 7679), 7680);
    }
}
