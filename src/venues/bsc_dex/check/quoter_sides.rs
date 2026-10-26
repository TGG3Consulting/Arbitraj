//! Файл: venues/bsc_dex/check/quoter_sides.rs
//! Что делает: итог сверки пула с квотером ПО КАЖДОЙ СТОРОНЕ (решение 73, решение Тиграна): покупка (USDT → монета)
//! и продажа (монета → USDT) сверяются независимо; сторона пула годна для котировки и сделки, только если прошла её
//! собственная сверка. Сторона сверяется, когда она глубокая; мелкая сторона — без итога (None), другая сторона от
//! этого не меняется.
//! Кто вызывает: check/state.rs (PoolFacts.quoter), check/quoter_cycle.rs (пишет), quote/select.rs, pools/depth_facts.rs,
//! check/cycle.rs, pools/listen_log.rs (читают).
//! От чего зависит: core/types (Side).
//! Настройки: нет.

use crate::core::types::Side;

/// Итог сверки одной стороны: None — ещё не сверялась (или сторона мелкая); Some(Err) — причина.
pub type SideCheck = Option<Result<(), String>>;

/// Сверка с квотером по сторонам.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuoterSides {
    pub buy: SideCheck,
    pub sell: SideCheck,
    /// Итог стороны — «не задана настройка»: её имя (GatewayError.setting там, где ошибка возникла — quoter_verify.rs verify) —
    /// сторона перепроверяется при задании именно её (решение 75).
    pub buy_wait: Option<String>,
    pub sell_wait: Option<String>,
}

impl QuoterSides {
    /// Итог стороны. Ошибок нет.
    pub fn side(&self, s: Side) -> &SideCheck {
        match s {
            Side::Buy => &self.buy,
            Side::Sell => &self.sell,
        }
    }

    /// Записывает итог стороны, другую не трогает. Ошибок нет.
    pub fn set(&mut self, s: Side, v: SideCheck) {
        match s {
            Side::Buy => self.buy = v,
            Side::Sell => self.sell = v,
        }
    }

    /// Записывает, какую настройку ждёт сторона (None — не ждёт). Ошибок нет.
    pub fn set_wait(&mut self, s: Side, v: Option<String>) {
        match s {
            Side::Buy => self.buy_wait = v,
            Side::Sell => self.sell_wait = v,
        }
    }

    /// Какая-то сторона ждёт одну из настроек `changed`. Ошибок нет.
    pub fn waits_for(&self, changed: &std::collections::HashSet<String>) -> bool {
        [&self.buy_wait, &self.sell_wait]
            .into_iter()
            .flatten()
            .any(|n| changed.contains(n))
    }

    /// Ни одна сторона ещё не сверялась. Ошибок нет.
    pub fn none(&self) -> bool {
        self.buy.is_none() && self.sell.is_none()
    }

    /// У какой-то стороны причина подходит под `f` (например «влияние крючка ещё не измерено»). Ошибок нет.
    pub fn any_err(&self, f: impl Fn(&str) -> bool) -> bool {
        [&self.buy, &self.sell]
            .into_iter()
            .any(|x| matches!(x, Some(Err(why)) if f(why)))
    }

    /// Сводно для сводки первого прохода: есть расхождение — оно; иначе прошла хоть одна — Ok; иначе None.
    /// Ошибок нет.
    pub fn combined(&self) -> SideCheck {
        for x in [&self.buy, &self.sell] {
            if let Some(Err(why)) = x {
                return Some(Err(why.clone()));
            }
        }
        (self.buy.is_some() || self.sell.is_some()).then_some(Ok(()))
    }
}
