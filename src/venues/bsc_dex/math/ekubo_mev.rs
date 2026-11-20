//! Файл: venues/bsc_dex/math/ekubo_mev.rs
//! Что делает: доплата расширения MEVCapture пула Ekubo на точный вход (решение 447): после обмена в ядре расширение
//! берёт с выхода дополнительную комиссию = комиссия пула × (|тик после − tickLast| / шаг тика). tickLast — тик пула
//! на начало секунды: если расширение в этой секунде ещё не обновлялось (lastUpdateTime ≠ время блока), tickLast = тик
//! пула до обмена. Время следующего блока заранее неизвестно: при lastUpdateTime = время блока чтения возможны оба
//! случая (тот же или следующий блок той же секунды — сохранённый tickLast; новая секунда — тик до обмена) — берётся
//! худший для нас (меньший выход). Раньше времени блока чтения — только «новая секунда».
//! Кто вызывает: quote/pool_quote.rs (ветка Ekubo).
//! От чего зависит: alloy-primitives, core/types (EkuboData), math/ekubo_sqrt.rs (compute_fee), math/ekubo_config.rs.
//! Настройки: нет.
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/extensions/MEVCapture.sol L177-L258 (handleForwardData: L183-L206
//! — tickLast и обновление в новой секунде, L208 — обмен в ядре, L211-L213 — доплата
//! min(uint64.max, ((|tick − tickLast| << 64) / tickSpacing · fee) >> 64), L232-L248 — точный вход: с выхода
//! computeFee(выход, доплата)); src/types/mevCapturePoolState.sol L10-L20.

use alloy_primitives::U256;

use crate::core::types::{EkuboData, EkuboMev};

use super::ekubo_config::{self, Shape};
use super::ekubo_sqrt::compute_fee;

/// Доплата (0.64) при заданном tickLast. Принимает: config пула, тик после обмена, tickLast. Ошибок нет (у не
/// концентрированного пула доплаты нет — MEVCapture берёт только концентрированные, MEVCapture.sol L64-L75).
pub fn additional_fee(d: &EkuboData, tick_after: i32, tick_last: i32) -> u64 {
    let Shape::Concentrated { spacing } = ekubo_config::shape(&d.config) else {
        return 0;
    };
    let fee = ekubo_config::fee(&d.config);
    let moved = (i64::from(tick_after) - i64::from(tick_last)).unsigned_abs();
    let mult = (U256::from(moved) << 64usize) / U256::from(spacing.max(1));
    let add = (mult * U256::from(fee)) >> 64usize;
    if add > U256::from(u64::MAX) {
        u64::MAX
    } else {
        add.to::<u64>()
    }
}

/// Выход после доплаты MEVCapture (худший из возможных tickLast следующего блока). Принимает: состояние, состояние
/// расширения, тик пула до обмена, тик после, выход ядра. Возвращает: выход владельцу. Ошибок нет.
pub fn out_after(d: &EkuboData, m: &EkuboMev, tick_before: i32, tick_after: i32, out: u128) -> u128 {
    let net = |last: i32| out - compute_fee(out, additional_fee(d, tick_after, last));
    let new_second = net(tick_before);
    if u64::from(m.last_update_time) >= d.block_ts {
        new_second.min(net(m.tick_last))
    } else {
        new_second
    }
}
