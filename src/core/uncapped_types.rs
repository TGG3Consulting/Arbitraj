//! Файл: core/uncapped_types.rs
//! Что делает: типы расчёта «без потолка» (решение 141): что было в бою (Battle — вердикт, бюджет, объём, прибыль,
//! id сделки), итог расчёта (Record — оптимум без денежных ограничений, минимальное вложение до плюса, лучшее на боевом
//! бюджете той же моделью, таблица потолков, упущено, что ограничило, состав затрат на оптимуме) и эпизод серии
//! одинаковых расчётов направления. Record пишется в SQLite (storage/uncapped_db.rs) JSON-ом; Brief — его часть для
//! итогов экрана (читается обратно).
//! Кто вызывает: core/uncapped.rs (собирает), uncapped_queue.rs, uncapped_store.rs, uncapped_view.rs, candidates.rs,
//! executor.rs (Battle).
//! От чего зависит: core/types, costs_types.rs (CostLine), limits_parts.rs, size_uncapped.rs (At, Level).
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::Direction;

use super::costs_types::CostLine;
use super::limits_parts::Parts;
use super::size_uncapped::{At, Level};

/// Что было в бою (на момент вердикта или входа в сделку).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Battle {
    /// accept — кандидат прошёл (сделка не запущена: сухой режим, монета занята…); reject — отказ по прибыли/порогу
    /// или финальной котировке; deal — сделка, в которую зашли (покупка исполнилась).
    pub verdict: String,
    pub reason: Option<String>,
    pub deal_id: Option<String>,
    pub dry: bool,
    /// Объём, списание по худшей цене и прибыль боевого расчёта (финальная котировка), USDT.
    pub qty: Decimal,
    pub invested: Decimal,
    pub profit: Decimal,
    /// Бюджет боевого подбора объёма (у сделки после подбора под остаток — свободный остаток); None — нет подбора.
    pub budget: Option<Decimal>,
    /// Только сделка: бюджет плана до чтения баланса и свободный остаток перед покупкой (решения 94, 103).
    pub plan_budget: Option<Decimal>,
    pub free: Option<Decimal>,
    /// Составляющие бюджета на момент решения (у сделки — до её открытия: своя сделка ещё не в учёте лимитов).
    pub parts: Option<Parts>,
    /// Решение 197: с какого момента грубый спред направления держится выше порога (core/signal.rs above_since,
    /// «Живой экран») на момент постановки — начало окна. У сделки — окно до входа: от него до исполнения покупки.
    /// None — спред сейчас не выше порога или записи до решения 197.
    #[serde(default)]
    pub window_from: Option<DateTime<Utc>>,
}

/// Эпизод: серия расчётов одного направления подряд (промежутки не длиннее uncapped.episode_gap_ms).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub first_at: DateTime<Utc>,
    pub last_at: DateTime<Utc>,
    /// Сколько расчётов в эпизоде и в скольких из них бюджет срезал оптимум.
    pub count: u64,
    pub capped_count: u64,
    /// Когда был пик упущенного (сама запись — пиковая).
    pub peak_at: DateTime<Utc>,
}

/// Итог одного расчёта без потолка (пиковый в эпизоде).
#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub at: DateTime<Utc>,
    pub direction: Direction,
    pub battle: Battle,
    /// Составляющие бюджета на момент расчёта (None — не прочитаны: причина в limits_error).
    pub parts: Option<Parts>,
    pub limits_error: Option<String>,
    /// Бюджет, на котором считалось «при нашем потолке» (та же модель, что без потолка).
    pub budget: Decimal,
    /// Что ограничило: per_trade / per_coin / in_transit / dry / free (свободный остаток) / budget (составляющие не
    /// прочитаны) — бюджет срезал оптимум; optimum — оптимум внутри бюджета; liquidity — край площадок;
    /// search_cap — предел поиска card.max_search_usdt.
    pub limited_by: String,
    pub min_profit_usd: Decimal,
    pub min_profit_pct: Decimal,
    pub opt: At,
    pub opt_passes: bool,
    pub opt_profit_pct: Decimal,
    pub opt_buy_avg: Decimal,
    pub opt_sell_avg: Decimal,
    pub opt_proceeds: Decimal,
    pub opt_costs_usdt: Decimal,
    pub opt_transit_s: Decimal,
    pub opt_costs: Vec<CostLine>,
    /// Наименьший объём, проходящий пороги; None — «не становится плюсовой» в пределах ликвидности и поиска.
    pub min_plus: Option<At>,
    pub at_budget: Level,
    pub levels: Vec<Level>,
    /// Упущено, USDT: см. uncapped.rs missed.
    pub missed: Decimal,
    pub upper_qty: Decimal,
    pub search_cap: Decimal,
    pub cap_reached: bool,
    pub limit: String,
    pub evals: u32,
    pub peaks: u32,
    pub capped: bool,
    /// Сколько длился расчёт, мс.
    pub ms: f64,
    pub ep: Episode,
}

/// Часть записи для итогов экрана (читается обратно из JSON).
#[derive(Debug, Clone, Deserialize)]
pub struct Brief {
    /// Время расчёта (у сделки — после исполнения покупки).
    pub at: DateTime<Utc>,
    pub direction: Direction,
    pub battle: Battle,
    pub limited_by: String,
    pub opt: At,
    pub opt_passes: bool,
    pub min_plus: Option<At>,
    pub at_budget: Level,
    pub levels: Vec<Level>,
    pub missed: Decimal,
    pub ep: Episode,
}

/// Бюджет срезал оптимум (а не оптимум внутри, не край площадок или поиска). Ошибок нет.
pub fn budget_bound(limited_by: &str) -> bool {
    !matches!(limited_by, "optimum" | "liquidity" | "search_cap")
}

/// Упущено, USDT. Прошёл / сделка: (прибыль без потолка, если проходит пороги, иначе 0) − (взяли бы на боевом бюджете
/// той же моделью), не меньше 0. Отказ («reject»): в бою взяли 0 — прибыль без потолка, если проходит пороги, иначе 0.
/// Принимает: вердикт боя, проходит ли оптимум, его прибыль, лучшее на боевом бюджете. Ошибок нет.
pub fn missed(verdict: &str, opt_passes: bool, opt_profit: Decimal, at_budget: &Level) -> Decimal {
    let opt = if opt_passes {
        opt_profit
    } else {
        Decimal::ZERO
    };
    if verdict == "reject" {
        opt
    } else {
        (opt - take(at_budget)).max(Decimal::ZERO)
    }
}

/// Прибыль, которую взяли бы на уровне: точка проходит пороги — её прибыль, иначе 0 (сделки не было бы). Ошибок нет.
pub fn take(l: &Level) -> Decimal {
    match (&l.at, l.passes) {
        (Some(a), true) => a.profit,
        _ => Decimal::ZERO,
    }
}
