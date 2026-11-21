//! Файл: core/types/pool_stable_debug.rs
//! Что делает: решение 472 — Debug для StableData: те же поля в том же порядке, что дал бы derive, а поля pair_d
//! (D_P пары одной строкой — Curve 2021) и ema (save_p после обмена) — только когда они true; wombat (решение 484) —
//! только когда Some. Так текстовые снимки
//! прежних видов (tests/data/multi_pair_2026-11-15/*.txt, «байт в байт») не меняются, а у Curve 2021 поля видны.
//! Кто вызывает: format!("{:?}") / {:#?} на StableData (журналы, снимки тестов).
//! От чего зависит: core/types/pool.rs (StableData).
//! Настройки: нет.

use std::fmt;

use super::pool::StableData;

impl fmt::Debug for StableData {
    /// Принимает: форматтер. Возвращает: результат записи. Ошибки: только форматтера.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("StableData");
        d.field("balances", &self.balances)
            .field("rates", &self.rates)
            .field("amp", &self.amp)
            .field("fee", &self.fee)
            .field("amp_precision", &self.amp_precision)
            .field("max_rounds", &self.max_rounds)
            .field("strict", &self.strict)
            .field("guard", &self.guard)
            .field("halted", &self.halted)
            .field("i0", &self.i0)
            .field("i1", &self.i1)
            .field("ng", &self.ng)
            .field("native", &self.native)
            .field("input_fee", &self.input_fee);
        if self.pair_d {
            d.field("pair_d", &self.pair_d);
        }
        if self.ema {
            d.field("ema", &self.ema);
        }
        // Решение 484: поле wombat — только у пула Wombat (снимки прежних видов прежние).
        if let Some(w) = &self.wombat {
            d.field("wombat", w);
        }
        d.finish()
    }
}
