//! Файл: core/uncapped_sum.rs
//! Что делает: сумма записей «без потолка» по группе (исполненные сделки или не исполненные кандидаты) для итогов экрана
//! «Упущено из-за потолка» (решения 141, 197): число, сколько упёрлось в бюджет, план, на нашем бюджете той же моделью,
//! потенциал без потолка, упущено, факт сделок из учёта, отклонённые плюсовые без потолка и (решение 197) суммарная
//! длительность окон: у кандидата — эпизод (первый → последний расчёт с упущенным), у сделки — от начала окна (спред
//! выше порога, Battle::window_from) до расчёта после покупки.
//! Решение 215: итоги периода целиком (Totals: обе группы, таблица «потолок → прибыль», топ направлений, вошедшие
//! записи) — одна функция totals у экрана (uncapped_view.rs) и отчёта Excel «Упущено» (report/missed_*.rs).
//! Решение 217: записи сделок, посчитанные после нашей покупки (до решения 217, Brief::understated), — отдельный итог
//! deals_old: их цифры занижены и с остальными не складываются (в таблицу потолков и топ не входят).
//! Кто вызывает: core/uncapped_view.rs, core/uncapped_report.rs.
//! От чего зависит: uncapped_types.rs, core/types (Direction).
//! Настройки: нет.

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::Direction;

use super::uncapped_types::{Brief, budget_bound, take};

/// Сумма по группе записей.
#[derive(Default)]
pub struct Sum {
    pub count: u64,
    pub bound: u64,
    pub battle: Decimal,
    pub at_budget: Decimal,
    pub potential: Decimal,
    pub missed: Decimal,
    pub fact: Decimal,
    pub fact_count: u64,
    pub plus_uncapped: u64,
    /// Решение 197: окон с известной длительностью и их сумма, с.
    pub windows: u64,
    pub window_s: i64,
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

/// Длительность окна записи, с: сделка — от начала окна (Battle::window_from) до момента решения о входе (решение 217:
/// decided_at; старые записи — до расчёта после покупки); кандидат — эпизод (первый → последний расчёт с упущенным).
/// None — у сделки начала окна нет. Ошибок нет.
pub fn window_s(b: &Brief) -> Option<i64> {
    if b.battle.verdict == "deal" {
        let end = deal_end(b);
        b.battle.window_from.map(|f| (end - f).num_seconds().max(0))
    } else {
        Some((b.ep.last_at - b.ep.first_at).num_seconds().max(0))
    }
}

/// Конец окна сделки: момент решения о входе (decided_at, решение 217); старые записи — время расчёта. Ошибок нет.
pub fn deal_end(b: &Brief) -> chrono::DateTime<chrono::Utc> {
    b.decided_at.unwrap_or(b.at)
}

/// Направление топа: монета (по адресу, решение 211) и площадки, сумма упущенного, сколько записей.
pub struct TopDir {
    pub dir: Direction,
    pub missed: Decimal,
    pub count: u64,
}

/// Запись, вошедшая в итог: тело, факт сделки из учёта (если известен), исполненная ли это сделка (группа «сделки»).
pub struct Item {
    pub b: Brief,
    pub fact: Option<Decimal>,
    pub deal: bool,
}

/// Итоги периода (решение 215: общие у экрана и отчёта «Упущено»).
#[derive(Default)]
pub struct Totals {
    pub deals: Sum,
    pub cands: Sum,
    /// Решение 217: сделки, посчитанные до решения 217 ПОСЛЕ нашей покупки (занижено) — отдельно, не смешиваются с
    /// deals; в таблицу потолков и топ не входят.
    pub deals_old: Sum,
    /// Потолок uncapped.ladder_usdt → «взяли бы» [сделки, кандидаты].
    pub ladder: Vec<(Decimal, [Decimal; 2])>,
    /// Все направления по убыванию упущенного (равные — больше записей выше).
    pub top: Vec<TopDir>,
    /// Решение 197: кандидаты с упущено 0 (старые записи) — в итог не входят, только счётчик.
    pub hidden: u64,
    /// Вошедшие записи в порядке выборки (новые сверху).
    pub items: Vec<Item>,
}

/// Итоги по записям периода. Принимает: записи (новые сверху), таблицу потолков, факт сделки по её id из учёта.
/// Возвращает: Totals. Ошибок нет.
pub fn totals(
    briefs: Vec<Brief>,
    ladder: &[Decimal],
    fact: impl Fn(&str) -> Option<Decimal>,
) -> Totals {
    let mut t = Totals {
        ladder: ladder.iter().map(|c| (*c, [Decimal::ZERO; 2])).collect(),
        ..Totals::default()
    };
    let mut by_dir: HashMap<Direction, (Decimal, u64)> = HashMap::new();
    for b in briefs {
        let g = usize::from(b.battle.verdict != "deal");
        // Решение 197: кандидаты с упущено 0 (записаны до решения) в итог не идут — только счётчик скрытых.
        if g == 1 && b.missed <= Decimal::ZERO {
            t.hidden += 1;
            continue;
        }
        let (deal, f) = match b.battle.deal_id.as_deref() {
            Some(id) if g == 0 => (true, fact(id)),
            _ => (false, None),
        };
        // Решение 217: старая запись сделки (посчитана после покупки — занижена) — свой итог, без таблицы и топа.
        if deal && b.understated() {
            t.deals_old.add(&b, f);
            t.items.push(Item { b, fact: f, deal });
            continue;
        }
        if deal {
            t.deals.add(&b, f);
        } else {
            t.cands.add(&b, None);
        }
        for (cap, sum) in t.ladder.iter_mut() {
            if let Some(l) = b.levels.iter().find(|l| l.cap == *cap) {
                sum[g] += take(l);
            }
        }
        let e = by_dir.entry(b.direction.clone()).or_default();
        e.0 += b.missed;
        e.1 += 1;
        t.items.push(Item { b, fact: f, deal });
    }
    t.top = by_dir
        .into_iter()
        .map(|(dir, (missed, count))| TopDir { dir, missed, count })
        .collect();
    t.top
        .sort_by(|a, b| b.missed.cmp(&a.missed).then(b.count.cmp(&a.count)));
    t
}
