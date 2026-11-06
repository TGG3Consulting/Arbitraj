//! Файл: report/rows.rs
//! Что делает: решение 193 — строки отчёта «Итоги сделок» (arb_results_<дата>.xlsx) из учёта сделок ядра: та же
//! выборка и те же значения, что в отчёте 05.11.2026 (scratchpad xl/build.py + deals.csv, сверено 06.11.2026 на
//! живых данных: 400 сделок, значения совпадают):
//! - сделки с вложением > 0 и итогом в учёте (result_usd); порядок — по времени старта;
//! - дата и время — старт сделки, UTC («ГГГГ-ММ-ДД», «ЧЧ:ММ»), час — число;
//! - длительность, с — от старта до последнего обновления записи, округление до целых;
//! - вложено — до 2 знаков, план и факт — до 4 знаков (как в deals.csv);
//! - площадки — человеческие имена (notify/human.rs venue: bingx → BingX, bsc_dex → DEX …).
//!
//! Кто вызывает: report/results_xlsx.rs.
//! От чего зависит: core/deal.rs (Deal), notify/human.rs (venue).
//! Настройки: нет.

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::deal::Deal;
use crate::notify::human::venue;

/// Строка листа «Сделки».
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub date: String,
    pub time: String,
    pub hour: u32,
    pub coin: String,
    pub buy: String,
    pub sell: String,
    pub invested: f64,
    pub plan: f64,
    pub fact: f64,
    pub duration_s: i64,
}

/// Число Decimal, округлённое до `dp` знаков, в f64. Ошибок нет (не влезло — 0).
fn num(d: Decimal, dp: u32) -> f64 {
    d.round_dp(dp).to_f64().unwrap_or(0.0)
}

/// Строки отчёта из сделок (см. шапку). Принимает: сделки в любом порядке. Ошибок нет.
pub fn rows(deals: &[Deal]) -> Vec<Row> {
    let mut v: Vec<&Deal> = deals
        .iter()
        .filter(|d| d.invested > Decimal::ZERO && d.result_usd.is_some())
        .collect();
    v.sort_by_key(|d| d.started_at);
    v.into_iter()
        .map(|d| Row {
            date: d.started_at.format("%Y-%m-%d").to_string(),
            time: d.started_at.format("%H:%M").to_string(),
            hour: d.started_at.format("%H").to_string().parse().unwrap_or(0),
            coin: d.direction.asset.ticker.clone(),
            buy: venue(&d.direction.buy_on.0),
            sell: venue(&d.direction.sell_on.0),
            invested: num(d.invested, 2),
            plan: num(d.plan_profit_usd, 4),
            fact: num(d.result_usd.unwrap_or_default(), 4),
            duration_s: (d.updated_at - d.started_at)
                .num_milliseconds()
                .saturating_add(500)
                / 1000,
        })
        .collect()
}
