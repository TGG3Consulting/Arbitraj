//! Файл: venues/bsc_dex/exec/quoter_refetch_ekubo.rs
//! Что делает: ревью решения 447 — сравнение состояния пула Ekubo в памяти и перечитанного с узла (решение 382) без
//! полей, которые зависят от МОМЕНТА чтения, а не от пула: время блока чтения (block_ts) и окно прочитанных слов
//! битмапа (words — вокруг текущего тика на момент чтения). Сравниваются config, цена, тик, ликвидность, состояние
//! MEVCapture, нативный ETH ключа, состояние TWAMM (время исполнения и скорости) и тики в ОБЩИХ прочитанных словах
//! (как карта тиков V3 — quoter_refetch_note.rs tick_diff).
//! Кто вызывает: exec/quoter_refetch_note.rs (state_differs).
//! От чего зависит: core/types (EkuboData), math/ekubo_bitmap.rs, math/ekubo_config.rs.
//! Настройки: нет.

use crate::core::types::{EkuboData, EkuboTick};

use super::super::math::ekubo_bitmap::word_index;
use super::super::math::ekubo_config::{self, Shape};

/// Тики в словах [lo, hi]. Ошибок нет.
fn ticks_in(d: &EkuboData, spacing: u32, lo: u64, hi: u64) -> Vec<EkuboTick> {
    let inside = |t: &&EkuboTick| (lo..=hi).contains(&word_index(t.tick, spacing).0);
    d.ticks.iter().filter(inside).copied().collect()
}

/// Совпадают ли состояния пула (см. шапку). Ошибок нет.
pub fn same(a: &EkuboData, b: &EkuboData) -> bool {
    let top = a.config == b.config
        && a.sqrt_ratio == b.sqrt_ratio
        && a.tick == b.tick
        && a.liquidity == b.liquidity
        && a.mev == b.mev
        && a.native == b.native;
    let twamm = match (&a.twamm, &b.twamm) {
        (Some(x), Some(y)) => (x.last, x.rate0, x.rate1) == (y.last, y.rate0, y.rate1),
        (None, None) => true,
        _ => false,
    };
    if !(top && twamm) {
        return false;
    }
    let Shape::Concentrated { spacing } = ekubo_config::shape(&a.config) else {
        return true;
    };
    let lo = a.words.0.max(b.words.0);
    let hi = a.words.1.min(b.words.1);
    lo > hi || ticks_in(a, spacing, lo, hi) == ticks_in(b, spacing, lo, hi)
}
