//! Файл: core/uncapped_record.rs
//! Что делает: сборка записи расчёта «без потолка» (решение 141, Record) из итога поиска size_uncapped.rs — общая для
//! живого расчёта (uncapped.rs compute) и пересчёта старых записей сделок по окнам (решение 218,
//! uncapped_recompute.rs): упущено (uncapped_types::missed), что ограничило (limited_by), оптимум и его затраты,
//! таблица потолков. Таблица потолков uncapped.ladder_usdt — ladder_in (по хранилищу настроек).
//! Вынесено из uncapped.rs, чтобы у обоих путей была одна сборка записи.
//! Кто вызывает: core/uncapped.rs, core/uncapped_recompute.rs.
//! От чего зависит: uncapped_types.rs, size_uncapped.rs, limits_parts.rs, costs_types.rs (Calc), settings.
//! Настройки: uncapped.ladder_usdt (ladder_in).

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::settings::read::At as SAt;
use crate::settings::store::Store;

use super::costs::Calc;
use super::limits_parts::Parts;
use super::size_uncapped::{At, Uncapped};
use super::uncapped_types::{Battle, Episode, Record, missed};

/// Всё о расчёте, кроме итога поиска.
pub struct Inputs {
    pub battle: Battle,
    pub parts: Option<Parts>,
    pub limits_error: Option<String>,
    /// Бюджет «при нашем потолке» (та же модель).
    pub budget: Decimal,
    pub min_usd: Decimal,
    pub min_pct: Decimal,
    pub search_cap: Decimal,
    /// Сколько длился расчёт, мс.
    pub ms: f64,
    /// Источник данных (uncapped_types BASIS_*), момент рынка и что снято.
    pub basis: String,
    pub decided_at: Option<DateTime<Utc>>,
    pub snapshot: Option<String>,
}

/// Что делает: запись из итога поиска. Принимает: входы, итог поиска, время записи. Возвращает: Record (эпизод — из
/// одного расчёта). Ошибок нет.
pub fn record(i: Inputs, u: Uncapped<Calc>, now: DateTime<Utc>) -> Record {
    let missed = missed(&i.battle.verdict, u.opt_passes, u.opt.profit, &u.at_budget);
    let limited_by = limited_by(&i.battle, &u, i.parts.as_ref(), i.budget);
    let c = &u.opt.data;
    Record {
        at: now,
        direction: c.direction.clone(),
        battle: i.battle,
        parts: i.parts,
        limits_error: i.limits_error,
        budget: i.budget,
        limited_by,
        min_profit_usd: i.min_usd,
        min_profit_pct: i.min_pct,
        opt: At::of(&u.opt),
        opt_passes: u.opt_passes,
        opt_profit_pct: c.profit_pct,
        opt_buy_avg: c.buy_avg,
        opt_sell_avg: c.sell_avg,
        opt_proceeds: c.proceeds,
        opt_costs_usdt: c.costs_usdt,
        opt_transit_s: c.transit_s,
        opt_costs: c.costs.clone(),
        min_plus: u.min_plus.as_ref().map(At::of),
        at_budget: u.at_budget,
        levels: u.levels,
        missed,
        upper_qty: u.upper_qty,
        search_cap: i.search_cap,
        cap_reached: u.cap_reached,
        limit: u.limit,
        evals: u.evals,
        peaks: u.peaks,
        capped: u.capped,
        ms: i.ms,
        ep: Episode {
            first_at: now,
            last_at: now,
            count: 1,
            capped_count: 0,
            peak_at: now,
        },
        basis: i.basis,
        decided_at: i.decided_at,
        snapshot: i.snapshot,
    }
}

/// Значения таблицы потолков (uncapped.ladder_usdt — список чисел USDT). Принимает: настройки.
/// Ошибки: не задана, не число, не больше 0.
pub fn ladder_in(s: &Store) -> Result<Vec<Decimal>, String> {
    let raw = s
        .list("uncapped.ladder_usdt", SAt::default())
        .map_err(|e| e.message)?;
    let mut v = raw
        .iter()
        .map(|x| match x.trim().parse::<Decimal>() {
            Ok(n) if n > Decimal::ZERO => Ok(n),
            _ => Err(format!(
                "uncapped.ladder_usdt: «{x}» — нужно число USDT больше 0"
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    v.sort_unstable();
    v.dedup();
    Ok(v)
}

/// Что ограничило (коды — uncapped_types.rs Record.limited_by). Ошибок нет.
pub fn limited_by<T>(
    b: &Battle,
    u: &Uncapped<T>,
    parts: Option<&Parts>,
    budget: Decimal,
) -> String {
    if u.opt.invested <= budget {
        let edge = u.opt.qty >= u.upper_qty;
        return match (edge, u.cap_reached) {
            (false, _) => "optimum",
            (true, true) => "search_cap",
            (true, false) => "liquidity",
        }
        .into();
    }
    // Сделка: свободный остаток перед покупкой меньше бюджета плана — объём подбирался под остаток (решение 103).
    if let (Some(free), Some(plan)) = (b.free, b.plan_budget)
        && free < plan
    {
        return "free".into();
    }
    parts.map_or("budget", |p| p.binding()).into()
}
