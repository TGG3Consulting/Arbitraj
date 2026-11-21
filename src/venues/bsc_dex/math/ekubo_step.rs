//! Файл: venues/bsc_dex/math/ekubo_step.rs
//! Что делает: следующий тик шага обмена в пуле Ekubo (решение 447) — граница шага цикла swap ядра: полный диапазон,
//! границы стейблсвапа или следующий тик битмапа концентрированного пула (skipAhead = 0).
//! Кто вызывает: math/ekubo_swap.rs.
//! От чего зависит: core/types (EkuboData), math/ekubo_bitmap.rs, ekubo_config.rs, ekubo_tick.rs.
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/Core.sol L576-L620 (стейблсвап L576-L602, концентрированный
//! L604-L619).

use crate::core::types::EkuboData;

use super::ekubo_bitmap;
use super::ekubo_config::Shape;
use super::ekubo_tick::{MAX_TICK, MIN_TICK};

/// Следующий тик шага: (тик, инициализирован, ликвидность шага активна). None — слово битмапа не прочитано.
/// Стейблсвап — Core.sol L576-L602, концентрированный — L604-L619 (skipAhead = 0). Ошибок нет.
pub fn next_step(
    d: &EkuboData,
    shape: Shape,
    tick: i32,
    increasing: bool,
) -> Option<(i32, bool, bool)> {
    match shape {
        Shape::FullRange => Some((if increasing { MAX_TICK } else { MIN_TICK }, false, true)),
        Shape::Stable { lower, upper } => {
            if (lower..upper).contains(&tick) {
                return Some((if increasing { upper } else { lower }, false, true));
            }
            if tick < lower {
                return Some((if increasing { lower } else { MIN_TICK }, false, false));
            }
            Some((if increasing { MAX_TICK } else { upper }, false, false))
        }
        Shape::Concentrated { spacing } => {
            let (t, i) = if increasing {
                ekubo_bitmap::next(d, tick, spacing)?
            } else {
                ekubo_bitmap::prev(d, tick, spacing)?
            };
            Some((t, i, true))
        }
    }
}
