//! Файл: report/cap_rows.rs
//! Что делает: решения 256, 257 — строки листа «Сделки» отчёта «Сделки, лимиты и капитал» из учёта сделок ядра.
//! Отбор строк (дополнение к 256 — аудит отчёта, п. 4 и 7): все сделки, которые касаются окна выгрузки — закончились в
//! нём (как «Итоги сделок»: конец — rows_time.rs ended_at, неизвестен — старт), начаты до его конца и ещё открыты, или
//! начаты раньше, а деньги покупки ещё заняты в окне (report/cap_hold.rs) — чтобы одновременность считалась и по ним.
//! «В итоге периода» — ровно отбор «Итогов сделок» (report/rows.rs selected: закончилась в окне и двигала деньги);
//! остальные строки — с причиной «почему не в итоге». Порядок — по началу, старые сверху.
//! Значения — факт учёта (core/deal.rs), без округления:
//! - покупка состоялась — у сделки есть исполненная покупка (Deal.buy: core/fills.rs, у цикла — cycle_fact.rs); у
//!   «Вернуть боту» («<id>r») покупки нет — «Нет» (п. 6 аудита);
//! - конец — запись закрытия в хронологии (ended_at — та же функция, что «Длительность» «Итогов сделок», решение 257);
//!   у открытой — пусто; у закрытой без записи конца — пусто и пометка «конец не найден» (п. 5);
//! - затраты (spent), затраты возврата USDT (return_cost_usd), факт (result_usd закрытой), оценка (provisional_usd
//!   остановленной и «Беру на себя», не факт) — как «Итоги сделок» (п. 3);
//! - статус и возврат USDT — report/rows_text.rs (status, usdt_return); занятость — cap_hold.rs; адреса — cap_addr.rs.
//!
//! Кто вызывает: report/cap_send.rs (capital_data), tests/report_capital.rs.
//! От чего зависит: core/deal.rs, report/rows.rs (selected, moves_money, f), rows_text.rs, rows_time.rs, cap_hold.rs,
//! cap_addr.rs, notify/human.rs (venue, route).
//! Настройки: нет.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};

use crate::core::deal::{Deal, Step};
use crate::notify::human::{route, venue};

use super::cap_addr::{self, Addr};
use super::cap_hold::{self, Hold};
use super::rows::{f, moves_money, selected};
use super::rows_text;
use super::rows_time::{Window, ended_at};

/// Что нужно строкам кроме сделок: id площадок DEX, кошельки DEX (id → имя; id → адрес или причина).
#[derive(Debug, Clone, Default)]
pub struct RowsCtx {
    pub dex: BTreeSet<String>,
    pub names: BTreeMap<String, String>,
    pub wallets: BTreeMap<String, Result<String, String>>,
}

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
    /// Пометка конца: «открыта», «конец не найден …»; пусто — конец есть.
    pub end_note: String,
    pub invested: f64,
    pub proceeds: f64,
    pub spent: f64,
    pub ret_cost: f64,
    pub plan: f64,
    pub fact: Option<f64>,
    pub estimate: Option<f64>,
    /// В итоге периода (отбор «Итогов сделок»).
    pub in_total: bool,
    /// Почему не в итоге; пусто — в итоге.
    pub why_not: String,
    pub ret: String,
    pub hold: Hold,
    pub addr: Addr,
}

/// Что делает: строки сделок окна (см. шапку), по началу. Принимает: все сделки, окно, контекст. Возвращает: строки.
/// Ошибок нет.
pub fn rows(deals: &[Deal], w: &Window, cx: &RowsCtx) -> Vec<DealRow> {
    let all: HashMap<&str, &Deal> = deals.iter().map(|d| (d.id.as_str(), d)).collect();
    let mut v: Vec<DealRow> = deals
        .iter()
        .filter_map(|d| {
            let hold = cap_hold::hold(d, &all, &cx.dex, &cx.names);
            touches(d, w, &hold).then(|| row(d, w, hold, cx))
        })
        .collect();
    v.sort_by(|a, b| a.start.cmp(&b.start).then(a.id.cmp(&b.id)));
    v
}

/// Касается ли сделка окна (см. шапку). Ошибок нет.
fn touches(d: &Deal, w: &Window, h: &Hold) -> bool {
    if w.to.is_some_and(|t| d.started_at >= t) {
        return false;
    }
    let from = |t: DateTime<Utc>| w.from.is_none_or(|f| t >= f);
    let key = ended_at(d).unwrap_or(d.started_at);
    d.is_open() || from(key) || (h.counted && h.until.is_none_or(from))
}

/// Почему сделка не в итоге периода; пусто — в итоге. Ошибок нет.
fn why_not(d: &Deal, w: &Window) -> String {
    if selected(d, w) {
        return String::new();
    }
    if d.is_open() {
        return "открыта — итога ещё нет".into();
    }
    if !moves_money(d) {
        return "не двигала деньги (как «Итоги сделок»)".into();
    }
    let key = ended_at(d).unwrap_or(d.started_at);
    if w.from.is_some_and(|f| key < f) {
        "закончилась до начала окна (здесь — для занятости денег)".into()
    } else {
        "закончилась после конца окна".into()
    }
}

/// Строка одной сделки (см. шапку). Ошибок нет.
fn row(d: &Deal, w: &Window, hold: Hold, cx: &RowsCtx) -> DealRow {
    let x = &d.direction;
    let end = ended_at(d);
    let end_note = match (d.is_open(), end) {
        (true, _) => "открыта".into(),
        (false, None) => "конец сделки в хронологии не найден".into(),
        _ => String::new(),
    };
    let closed = d.step == Step::Closed;
    DealRow {
        id: d.id.clone(),
        coin: x.asset.ticker.clone(),
        contract: (x.asset.bsc_contract)
            .map(|a| format!("{a:#x}"))
            .unwrap_or_default(),
        buy: venue(&x.buy_on.0),
        sell: venue(&x.sell_on.0),
        dir: route(&x.buy_on.0, &x.sell_on.0),
        bought: d.buy.is_some(),
        status: rows_text::status(d),
        start: d.started_at,
        end,
        end_note,
        invested: f(d.invested),
        proceeds: f(d.proceeds),
        spent: f(d.spent),
        ret_cost: f(d.return_cost_usd.unwrap_or_default()),
        plan: f(d.plan_profit_usd),
        fact: if closed { d.result_usd.map(f) } else { None },
        estimate: match d.step {
            Step::Halted | Step::TakenOver => Some(f(d.provisional_usd.unwrap_or_default())),
            _ => None,
        },
        in_total: selected(d, w),
        why_not: why_not(d, w),
        ret: rows_text::usdt_return(d),
        hold,
        addr: cap_addr::of(d, &cx.wallets),
    }
}
