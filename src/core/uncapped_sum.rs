//! Файл: core/uncapped_sum.rs
//! Что делает: сумма записей «без потолка» по группе (исполненные сделки или не исполненные кандидаты) для итогов экрана
//! «Упущено из-за потолка» (решения 141, 197): число, сколько упёрлось в бюджет, план, на нашем бюджете той же моделью,
//! потенциал без потолка, упущено, факт сделок из учёта, отклонённые плюсовые без потолка и (решение 197) суммарная
//! длительность окон: у кандидата — эпизод (первый → последний расчёт с упущенным), у сделки — от начала окна (спред
//! выше порога, Battle::window_from) до расчёта после покупки.
//! Кто вызывает: core/uncapped_view.rs.
//! От чего зависит: uncapped_types.rs.
//! Настройки: нет.

use rust_decimal::Decimal;
use serde_json::{Value, json};

use super::uncapped_types::{Brief, budget_bound, take};

/// Сумма по группе записей.
#[derive(Default)]
pub struct Sum {
    count: u64,
    bound: u64,
    battle: Decimal,
    at_budget: Decimal,
    pub potential: Decimal,
    missed: Decimal,
    fact: Decimal,
    fact_count: u64,
    plus_uncapped: u64,
    /// Решение 197: окон с известной длительностью и их сумма, с.
    windows: u64,
    window_s: i64,
}

impl Sum {
    /// Добавляет запись (fact — итог сделки, если известен). Ошибок нет.
    pub fn add(&mut self, b: &Brief, fact: Option<Decimal>) {
        self.count += 1;
        self.bound += u64::from(budget_bound(&b.limited_by));
        self.battle += b.battle.profit;
        self.at_budget += take(&b.at_budget);
        if b.opt_passes {
            self.potential += b.opt.profit;
        }
        self.missed += b.missed;
        if let Some(f) = fact {
            self.fact += f;
            self.fact_count += 1;
        }
        self.plus_uncapped += u64::from(b.battle.verdict == "reject" && b.opt_passes);
        if let Some(s) = window_s(b) {
            self.windows += 1;
            self.window_s += s;
        }
    }

    /// JSON для экрана. Ошибок нет.
    pub fn json(&self) -> Value {
        json!({
            "count": self.count, "budget_bound": self.bound, "battle_profit": self.battle,
            "at_budget_profit": self.at_budget, "potential_profit": self.potential, "missed": self.missed,
            "fact": self.fact, "fact_count": self.fact_count, "plus_uncapped": self.plus_uncapped,
            "windows": self.windows, "window_s": self.window_s,
        })
    }
}

/// Длительность окна записи, с: сделка — от начала окна (Battle::window_from) до расчёта после покупки; кандидат —
/// эпизод (первый → последний расчёт с упущенным). None — у сделки начала окна нет. Ошибок нет.
pub fn window_s(b: &Brief) -> Option<i64> {
    if b.battle.verdict == "deal" {
        b.battle
            .window_from
            .map(|f| (b.at - f).num_seconds().max(0))
    } else {
        Some((b.ep.last_at - b.ep.first_at).num_seconds().max(0))
    }
}
