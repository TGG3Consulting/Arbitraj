//! Файл: report/rows.rs
//! Что делает: решения 193, 257 — строки листа «Сделки» отчёта «Итоги сделок» (arb_results_<дата>.xlsx) из учёта ядра.
//! Отбор (решение 257, Тигран 11.11 п. 4): сделки, которые закончились в окне выгрузки (конец — rows_time.rs ended_at:
//! закрыта, остановлена, «Беру на себя»; конец неизвестен — по старту) и двигали деньги (вложено, выручка, затраты, итог
//! или оценка не ноль — в том числе вложение 0 с затратами). Открытые — не здесь, а на листе «Слепок по балансам».
//! Значения — без округления (формат ячеек — 4 знака): факт — result_usd (выручка − вложено − затраты − затраты
//! возврата USDT, core/ledger.rs, deal_return_cost.rs); оценка — provisional_usd остановленной и «Беру на себя»
//! (решение 152), явно отдельно от факта; план — plan_profit_usd (core/costs.rs: уже за вычетом запаса на движение
//! цены и, в режиме «в каждой сделке», затрат возврата USDT); запас и возврат плана — rows_text.rs plan_parts.
//! Затраты по статьям — Deal.cost_items (core/deal_book.rs); разница spent − Σ статей — «без разбивки» (сделки до 257).
//! Порядок — по концу сделки. Монета — тикер; одинаковый тикер у разных адресов — с адресом (coin_labels).
//! Кто вызывает: report/results_xlsx.rs (через report/mod.rs); report/cap_rows.rs (selected, moves_money, f —
//! решение 256).
//! От чего зависит: core/deal.rs, core/deal_return_cost.rs, report/rows_text.rs, report/rows_time.rs, notify/human.rs.
//! Настройки: нет.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Timelike, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::core::deal::{Deal, Step};
use crate::notify::human::venue;

use super::rows_text as tx;
use super::rows_time::{Window, ended_at};

/// Строка листа «Сделки».
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    pub start: DateTime<Utc>,
    /// Конец сделки (rows_time.rs); None — в хронологии не найден.
    pub end: Option<DateTime<Utc>>,
    /// День и час для сводок: конца, а если он неизвестен — старта.
    pub date: String,
    pub hour: u32,
    pub coin: String,
    pub buy: String,
    pub sell: String,
    /// Направление итога — пара «площадка × тип направления» (rows_text.rs owner_pair, решение 258), подпись для человека.
    pub owner: String,
    pub status: String,
    pub invested: f64,
    pub proceeds: f64,
    pub spent: f64,
    /// Затраты возврата USDT, вошедшие в факт (return_cost_usd).
    pub ret_fact: f64,
    pub fact: Option<f64>,
    pub estimate: Option<f64>,
    pub plan: f64,
    /// (запас на движение цены, возврат USDT) плана; None — у плана нет строк расчёта.
    pub plan_parts: Option<(f64, f64)>,
    pub duration_s: Option<i64>,
    pub usdt_return: String,
    /// Статьи затрат по факту (подпись вида — rows_text.rs cost_label → USDT).
    pub costs: BTreeMap<&'static str, f64>,
    /// spent − Σ статей (сделки до решения 257).
    pub unsplit: f64,
}

/// Decimal → f64 без округления. Ошибок нет (не влезло — 0).
pub fn f(d: Decimal) -> f64 {
    d.to_f64().unwrap_or(0.0)
}

/// Двигала ли сделка деньги (см. шапку). Ошибок нет.
pub fn moves_money(d: &Deal) -> bool {
    let z = |x: Option<Decimal>| x.unwrap_or_default().is_zero();
    !(d.invested.is_zero()
        && d.proceeds.is_zero()
        && d.spent.is_zero()
        && z(d.result_usd)
        && z(d.provisional_usd))
}

/// Отбор сделки в отчёт (см. шапку): закончилась (конец неизвестен — старт) в окне и двигала деньги. Одна функция для
/// «Итогов сделок» и итога периода «Сделок, лимитов и капитала» (решение 256, дополнение). Ошибок нет.
pub fn selected(d: &Deal, w: &Window) -> bool {
    !d.is_open() && moves_money(d) && w.has(ended_at(d).unwrap_or(d.started_at))
}

/// Строки отчёта. Принимает: сделки, окно, id площадок DEX. Возвращает: строки по концу сделки. Ошибок нет.
pub fn rows(deals: &[Deal], w: &Window, dex: &BTreeSet<String>) -> Vec<Row> {
    let labels = coin_labels(deals);
    let mut v: Vec<(DateTime<Utc>, Row)> = deals
        .iter()
        .filter(|d| selected(d, w))
        .map(|d| {
            let end = ended_at(d);
            let key = end.unwrap_or(d.started_at);
            (key, row(d, end, key, dex, &labels))
        })
        .collect();
    v.sort_by_key(|(k, _)| *k);
    v.into_iter().map(|(_, r)| r).collect()
}

/// Одна строка. Ошибок нет.
fn row(
    d: &Deal,
    end: Option<DateTime<Utc>>,
    key: DateTime<Utc>,
    dex: &BTreeSet<String>,
    labels: &BTreeMap<String, String>,
) -> Row {
    let mut costs: BTreeMap<&'static str, f64> = BTreeMap::new();
    let mut split = Decimal::ZERO;
    for c in &d.cost_items {
        *costs.entry(tx::cost_label(c.kind)).or_default() += f(c.usd);
        split += c.usd;
    }
    let mut status = tx::status(d);
    if end.is_none() {
        status.push_str(" (конец сделки в хронологии не найден — день по старту)");
    }
    let closed = d.step == Step::Closed;
    Row {
        id: d.id.clone(),
        start: d.started_at,
        end,
        date: key.format("%Y-%m-%d").to_string(),
        hour: key.hour(),
        coin: labels.get(&d.id).cloned().unwrap_or_default(),
        buy: venue(&d.direction.buy_on.0),
        sell: venue(&d.direction.sell_on.0),
        owner: tx::owner_pair(d, dex),
        status,
        invested: f(d.invested),
        proceeds: f(d.proceeds),
        spent: f(d.spent),
        ret_fact: f(d.return_cost_usd.unwrap_or_default()),
        fact: if closed { d.result_usd.map(f) } else { None },
        estimate: if closed {
            None
        } else {
            Some(f(d.provisional_usd.unwrap_or_default()))
        },
        plan: f(d.plan_profit_usd),
        plan_parts: tx::plan_parts(&d.plan).map(|(r, x)| (f(r), f(x))),
        // Ровно «Конец − Старт» по показанным значениям (до секунды, доли отброшены — как в колонках B и C).
        duration_s: end.map(|e| e.timestamp() - d.started_at.timestamp()),
        usdt_return: tx::usdt_return(d),
        costs,
        unsplit: f(d.spent - split),
    }
}

/// Подписи монет по id сделки: тикер; тикер, под которым в учёте несколько адресов, — «тикер 0x…» (адрес
/// контракта BSC), чтобы разные монеты не склеились в «По монетам». Ошибок нет.
pub fn coin_labels(deals: &[Deal]) -> BTreeMap<String, String> {
    let mut addrs: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for d in deals {
        let a = &d.direction.asset;
        let key = a.bsc_contract.map(|c| format!("{c:?}")).unwrap_or_default();
        addrs.entry(a.ticker.as_str()).or_default().insert(key);
    }
    deals
        .iter()
        .map(|d| {
            let a = &d.direction.asset;
            let many = addrs.get(a.ticker.as_str()).is_some_and(|s| s.len() > 1);
            let label = match (many, a.bsc_contract) {
                (true, Some(c)) => format!("{} {c:?}", a.ticker),
                _ => a.ticker.clone(),
            };
            (d.id.clone(), label)
        })
        .collect()
}
