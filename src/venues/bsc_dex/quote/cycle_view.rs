//! Файл: venues/bsc_dex/quote/cycle_view.rs
//! Что делает: перевод сигнала цикла адаптера (quote/cycle.rs: CycleSignal) в read-тип ядра (core/types/dex_cycle.rs:
//! DexCycle) для контракта площадки (Venue::dex_cycles) и «Живого экрана». Только отображение: ничего не считает и не
//! исполняет — переносит уже посчитанные поля один-в-один и подставляет id площадки (стык образца решения 74).
//! Кто вызывает: venues/bsc_dex/venue.rs (Venue::dex_cycles); тесты (tests/bsc_dex_cycle.rs — маппинг на реальной записи).
//! От чего зависит: quote/cycle.rs (CycleSignal, CycleHop), core/types (DexCycle, DexCycleHop, VenueId).
//! Настройки: нет.

use crate::core::types::{DexCycle, DexCycleHop, VenueId};

use super::cycle::{CycleHop, CycleSignal};

/// Что делает: переводит сигнал цикла в read-тип ядра, добавляя площадку. Принимает: id площадки, сигнал.
/// Возвращает: DexCycle (поля сигнала один-в-один). Ошибок нет.
pub fn to_dex_cycle(venue: &VenueId, s: &CycleSignal) -> DexCycle {
    DexCycle {
        venue: venue.clone(),
        base: s.base,
        base_ticker: s.base_ticker.clone(),
        input: s.input,
        output: s.output,
        gross_profit: s.gross_profit,
        gas_units: s.gas_units,
        gas_bnb: s.gas_bnb,
        tip_bnb: s.tip_bnb,
        bnb_cost: s.bnb_cost,
        bnb_cost_in_base: s.bnb_cost_in_base,
        net_profit: s.net_profit,
        hops: s.hops.iter().map(hop).collect(),
        executable: s.executable,
        exec_note: s.exec_note.clone(),
        block: s.block,
    }
}

/// Хоп сигнала → хоп ядра (один-в-один). Ошибок нет.
fn hop(h: &CycleHop) -> DexCycleHop {
    DexCycleHop {
        index: h.index,
        family: h.family.clone(),
        kind: h.kind,
        pool: h.pool.clone(),
        token_in: h.token_in,
        token_out: h.token_out,
        fee_pct: h.fee_pct,
        hook_pct: h.hook_pct,
        gas_units: h.gas_units,
    }
}
