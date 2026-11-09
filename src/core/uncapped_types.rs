//! Файл: core/uncapped_types.rs
//! Что делает: типы расчёта «без потолка» (решение 141): что было в бою (Battle — вердикт, бюджет, объём, прибыль,
//! id сделки), итог расчёта (Record — оптимум без денежных ограничений, минимальное вложение до плюса, лучшее на боевом
//! бюджете той же моделью, таблица потолков, упущено, что ограничило, состав затрат на оптимуме) и эпизод серии
//! одинаковых расчётов направления. Record пишется в SQLite (storage/uncapped_db.rs) JSON-ом; Brief — его часть для
//! итогов экрана (читается обратно). Решение 217: источник данных записи (basis) — снимок рынка момента решения у
//! сделки, живые котировки у кандидата; старые записи сделок без поля — «после покупки» (занижено). Решение 237:
//! новые записи сделок — BASIS_DECISION (все входы на момент входа); BASIS_SNAPSHOT — записи до него (σ и курсы позже).
//! Решение 239: в итогах (экран, отчёт «Упущено») сделки складываются только с BASIS_DECISION; любые другие записи
//! сделок (decision_snapshot, live_after_buy, пересчёт по окну решения 218) — «несравнимо», своей строкой (incomparable).
//! Кто вызывает: core/uncapped.rs (собирает), uncapped_queue.rs, uncapped_store.rs, uncapped_view.rs, candidates.rs,
//! executor.rs (Battle), uncapped_sum.rs и report/missed_rows.rs (Brief, решение 215).
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
    /// Решение 239: объёмы, недопустимые из-за валюты затрат не из плана сделки (core/plan_gap.rs) — сколько и первая
    /// причина; None — таких не было.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_gap: Option<String>,
    pub evals: u32,
    pub peaks: u32,
    pub capped: bool,
    /// Сколько длился расчёт, мс.
    pub ms: f64,
    pub ep: Episode,
    /// Решение 217: на каких данных посчитано — BASIS_SNAPSHOT (сделка: снимок рынка момента решения, до ордера
    /// покупки), BASIS_LIVE (кандидат: живые котировки в момент расчёта), BASIS_LIVE_AFTER_BUY (сделка до решения 217:
    /// живые котировки ПОСЛЕ нашей покупки — занижено; так читаются старые записи); решение 218 — BASIS_RECOMPUTED и
    /// BASIS_RECOMPUTED_DEX_PLAN: старая запись пересчитана по окну сделки (новая запись, последняя по сделке).
    pub basis: String,
    /// Решение 217: когда снят рынок (момент решения, перед ордером покупки) и что снято по сторонам; без снимка — None.
    pub decided_at: Option<DateTime<Utc>>,
    pub snapshot: Option<String>,
}

/// Решение 217: сделка посчитана по снимку рынка момента решения (стаканы и пулы), но σ, время в пути, курсы затрат и
/// контекст направления — на момент расчёта (до решения 237): с решением о входе несравнимо. Новые записи так не пишутся.
pub const BASIS_SNAPSHOT: &str = "decision_snapshot";
/// Решение 237: сделка посчитана целиком по данным момента входа — стаканы и пулы снимка, σ и T плана сделки, курсы
/// затрат и затраты DEX-стороны из плана, контекст направления на момент решения, цена газа DEX на момент снимка.
pub const BASIS_DECISION: &str = "decision_inputs";

/// Решение 237: запись сделки посчитана по снимку стаканов, но с σ и курсами момента расчёта (BASIS_SNAPSHOT) —
/// несравнима с решением о входе. Ошибок нет.
pub fn late_inputs(verdict: &str, basis: &str) -> bool {
    verdict == "deal" && basis == BASIS_SNAPSHOT
}
/// Решение 217: живые котировки в момент расчёта (кандидаты: нашей сделки не было).
pub const BASIS_LIVE: &str = "live";
/// Решение 217: старая запись сделки — живые котировки после нашей покупки (занижено).
pub const BASIS_LIVE_AFTER_BUY: &str = "live_after_buy";

/// Решение 218: старая запись сделки пересчитана по окну сделки — обе стороны по стаканам бирж окна на момент решения.
pub const BASIS_RECOMPUTED: &str = "recomputed_window";
/// Решение 218: старая запись сделки биржа → DEX пересчитана: биржа — по стакану окна, DEX — из плана (приблизительно).
pub const BASIS_RECOMPUTED_DEX_PLAN: &str = "recomputed_window_dex_plan";

/// Решение 218: запись — пересчёт старой записи по окну (новая запись сделки, старая не удалена). Ошибок нет.
pub fn recomputed(basis: &str) -> bool {
    basis == BASIS_RECOMPUTED || basis == BASIS_RECOMPUTED_DEX_PLAN
}

/// Источник данных записи без поля basis (записи до решения 217) — «после покупки» (serde default). Ошибок нет.
pub fn old_basis() -> String {
    BASIS_LIVE_AFTER_BUY.into()
}

/// Решение 217: запись сделки посчитана после нашей покупки — занижена (её цифры нельзя смешивать с остальными).
/// Принимает: вердикт боя, источник. Ошибок нет.
pub fn understated(verdict: &str, basis: &str) -> bool {
    verdict == "deal" && basis == BASIS_LIVE_AFTER_BUY
}

/// Решение 239: запись сделки посчитана НЕ по данным момента входа (basis не BASIS_DECISION: decision_snapshot,
/// live_after_buy, пересчёт по окну решения 218 — там настройки модели текущие, а не на момент сделки) — в итогах
/// экрана и отчёта «Упущено» с новыми не складывается, своя строка «несравнимо». Принимает: вердикт боя, источник.
/// Ошибок нет.
pub fn incomparable(verdict: &str, basis: &str) -> bool {
    verdict == "deal" && basis != BASIS_DECISION
}

/// Часть записи для итогов экрана (читается обратно из JSON).
#[derive(Debug, Clone, Deserialize)]
pub struct Brief {
    /// Время расчёта (у сделки — после исполнения покупки; рынок — на decided_at, решение 217).
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
    /// Решение 215 (отчёт «Упущено», лист «Кандидаты»): бюджет «на нашем бюджете» (Record::budget).
    #[serde(default)]
    pub budget: Decimal,
    /// Решение 217: источник данных (Record::basis); старые записи — «после покупки».
    #[serde(default = "old_basis")]
    pub basis: String,
    /// Решение 217: момент решения о входе (снимок рынка); старые записи — None.
    #[serde(default)]
    pub decided_at: Option<DateTime<Utc>>,
    /// Решение 245: окна максимального объёма эпизода кандидата (uncapped_maxwin.rs); старые записи и сделки — None.
    #[serde(default)]
    pub maxwin: Option<super::uncapped_maxwin::MaxWins>,
}

impl Brief {
    /// Решение 217: запись сделки посчитана после нашей покупки — занижена. Ошибок нет.
    pub fn understated(&self) -> bool {
        understated(&self.battle.verdict, &self.basis)
    }

    /// Решение 239: запись сделки не по данным момента входа — «несравнимо» (incomparable). Ошибок нет.
    pub fn incomparable(&self) -> bool {
        incomparable(&self.battle.verdict, &self.basis)
    }
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
