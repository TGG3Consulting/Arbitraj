//! Файл: report/cap_rows.rs
//! Что делает: решение 256 — строки листа «Сделки» отчёта «Сделки, лимиты и капитал» из учёта сделок ядра: ВСЕ сделки,
//! начатые в периоде (по started_at, как «Итоги сделок» решения 215), по строке на сделку, старые сверху. Значения —
//! факт учёта (core/deal.rs):
//! - покупка состоялась — вложено > 0 (вложено растёт только по исполненной покупке: core/fills.rs trade);
//! - статус — шаг сделки по-русски (core/deal_text.rs step_name);
//! - конец — время закрытия сделки по хронологии (report/rows_time.rs ended_at — та же функция, что «Длительность»
//!   «Итогов сделок»; Тигран 11.11, решение 257); не найдено — пусто;
//!   у открытой сделки (шаг не «закрыта» / «остановлена» / «монеты у человека», Deal::is_open) — пусто;
//! - итог по факту — result_usd (пусто — итога ещё нет);
//! - возврат USDT — состояние каждой части возврата выручки на биржу покупки (core/usdt_return_state.rs) через «; ».
//!
//! Кто вызывает: report/mod.rs (capital_send), tests/report_capital.rs.
//! От чего зависит: core/deal.rs, core/deal_text.rs, core/usdt_return_state.rs, notify/human.rs (venue, route), report/rows_time.rs (ended_at).
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::deal::Deal;
use crate::core::deal_text::step_name;
use crate::core::usdt_return_state::UsdtReturn;
use crate::notify::human::{route, venue};

use super::in_period;

/// Строка листа «Сделки».
#[derive(Debug, Clone, PartialEq)]
pub struct DealRow {
    pub id: String,
    pub coin: String,
    /// Адрес контракта BSC; пусто — площадка не сообщила адрес.
    pub contract: String,
    pub buy: String,
    pub sell: String,
    /// «MEXC → DEX».
    pub dir: String,
    pub bought: bool,
    pub status: String,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    pub invested: f64,
    pub proceeds: f64,
    pub plan: f64,
    pub fact: Option<f64>,
    pub ret: String,
}

/// Decimal в f64 без округления (формат ячейки — 2 знака). Ошибок нет (не влезло — 0).
pub fn f(d: Decimal) -> f64 {
    d.to_f64().unwrap_or(0.0)
}

/// Что делает: строки сделок периода [from, to) по started_at, старые сверху. Принимает: сделки в любом порядке,
/// границы. Возвращает: строки. Ошибок нет.
pub fn rows(
    deals: &[Deal],
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> Vec<DealRow> {
    let mut v: Vec<&Deal> = deals
        .iter()
        .filter(|d| in_period(d.started_at, from, to))
        .collect();
    v.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
    v.into_iter().map(row).collect()
}

/// Строка одной сделки (см. шапку). Ошибок нет.
pub fn row(d: &Deal) -> DealRow {
    let x = &d.direction;
    DealRow {
        id: d.id.clone(),
        coin: x.asset.ticker.clone(),
        contract: x
            .asset
            .bsc_contract
            .map(|a| format!("{a:#x}"))
            .unwrap_or_default(),
        buy: venue(&x.buy_on.0),
        sell: venue(&x.sell_on.0),
        dir: route(&x.buy_on.0, &x.sell_on.0),
        bought: d.invested > Decimal::ZERO,
        status: step_name(d.step).into(),
        start: d.started_at,
        // Решение 257 (Тигран 11.11): конец — закрытие по хронологии, как в «Итогах сделок».
        end: super::rows_time::ended_at(d),
        invested: f(d.invested),
        proceeds: f(d.proceeds),
        plan: f(d.plan_profit_usd),
        fact: d.result_usd.map(f),
        ret: usdt_return(d),
    }
}

/// Состояние возврата USDT сделки: части через «; »; нет записи возврата — «—». Ошибок нет.
pub fn usdt_return(d: &Deal) -> String {
    let parts: Vec<String> = d
        .usdt_return
        .iter()
        .chain(d.usdt_return_parts.iter())
        .map(part)
        .collect();
    if !parts.is_empty() {
        return parts.join("; ");
    }
    match &d.usdt_return_skip {
        Some(why) => format!("не делается: {why}"),
        None => "—".into(),
    }
}

/// Одна часть возврата по-русски с суммой и площадками. Ошибок нет.
fn part(r: &UsdtReturn) -> String {
    let n = |x: &Decimal| crate::notify::human::usd(*x);
    match r {
        UsdtReturn::Pending { amount, from, to } => {
            format!("идёт: {} {}", n(amount), route(&from.0, &to.0))
        }
        UsdtReturn::Retrying {
            amount, from, to, ..
        } => format!("ждёт повтора: {} {}", n(amount), route(&from.0, &to.0)),
        UsdtReturn::Done {
            credited, from, to, ..
        } => format!("вернулись: {} {}", n(credited), route(&from.0, &to.0)),
        UsdtReturn::NotReturned { amount, at, .. } => {
            format!("не вернулись: {} на {}", n(amount), venue(&at.0))
        }
        UsdtReturn::Manual { amount, at, to, .. } => {
            format!("перегнано вручную: {} {}", n(amount), route(&at.0, &to.0))
        }
    }
}
