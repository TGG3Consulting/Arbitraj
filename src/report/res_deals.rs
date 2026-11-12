//! Файл: report/res_deals.rs
//! Что делает: решение 257 — лист «Сделки» отчёта «Итоги сделок»: строка на сделку (report/rows.rs), колонки — ниже
//! (C — буквы для формул других листов). Значения — из учёта сделки; формулы: направление, плюс/минус, план до запаса
//! без возврата USDT, факт без возврата USDT, «факт − план» (одинаковый состав: оба до запаса на движение цены и без
//! затрат возврата USDT). Колонки статей затрат — по видам, что есть в строках (rows_text.rs cost_label), и «без
//! разбивки» (сделки до решения 257); сумма статей + без разбивки = «Затраты сделки».
//! Кто вызывает: report/results_xlsx.rs; results_sheet.rs, res_days.rs (C, rng).
//! От чего зависит: rust_xlsxwriter, report/rows.rs, report/rows_text.rs, report/results_sheet.rs.
//! Настройки: нет.

use std::collections::BTreeSet;

use rust_xlsxwriter::{Formula, Worksheet, XlsxError};

use super::results_sheet::{base, header, money, money4, red_green};
use super::rows::Row;

/// Буквы колонок листа «Сделки» (для формул сводок).
pub struct Cols {
    pub date: &'static str,
    pub hour: &'static str,
    pub coin: &'static str,
    pub dir: &'static str,
    pub owner: &'static str,
    pub invested: &'static str,
    pub ret_fact: &'static str,
    pub fact: &'static str,
    pub estimate: &'static str,
    pub plan: &'static str,
    pub reserve: &'static str,
    pub plan_ret: &'static str,
    pub cmp: &'static str,
    pub duration: &'static str,
}

/// Буквы колонок (порядок — HDR).
pub const C: Cols = Cols {
    date: "D",
    hour: "E",
    coin: "F",
    dir: "I",
    owner: "J",
    invested: "L",
    ret_fact: "O",
    fact: "P",
    estimate: "Q",
    plan: "S",
    reserve: "T",
    plan_ret: "U",
    cmp: "X",
    duration: "Y",
};

/// Шапка постоянных колонок A…Z.
const HDR: [&str; 26] = [
    "Сделка (id)",
    "Старт (UTC)",
    "Конец (UTC)",
    "День конца (UTC)",
    "Час конца (UTC)",
    "Монета",
    "Купили на",
    "Продали на",
    "Направление",
    "Направление итога (площадка × тип)",
    "Статус",
    "Вложено, USDT",
    "Выручка, USDT",
    "Затраты сделки, USDT",
    "Затраты возврата USDT (факт), USDT",
    "Факт, USDT",
    "Оценка (не факт), USDT",
    "Плюс/минус",
    "План, USDT",
    "Запас на движение цены (план), USDT",
    "Возврат USDT в плане, USDT",
    "План до запаса, без возврата USDT",
    "Факт без возврата USDT",
    "Факт − план (до запаса, без возврата USDT)",
    "Длительность (старт → конец), с",
    "Возврат USDT (состояние)",
];

/// Диапазон колонки листа «Сделки» для формул: Сделки!$P$2:$P$n (n ≥ 2: без строк — пустая строка 2).
pub fn rng(c: &str, n: u32) -> String {
    format!("Сделки!${c}$2:${c}${}", n.max(2))
}

/// Лист «Сделки». Принимает: лист, строки. Ошибки: XlsxError.
pub fn sheet(ws: &mut Worksheet, rows: &[Row]) -> Result<(), XlsxError> {
    ws.set_name("Сделки")?;
    let kinds: BTreeSet<&str> = rows.iter().flat_map(|r| r.costs.keys().copied()).collect();
    let mut hdr: Vec<&str> = HDR.to_vec();
    hdr.extend(kinds.iter().copied());
    hdr.push("Затраты без разбивки (сделки до решения 257), USDT");
    let mut w = vec![
        18., 19., 19., 12., 8., 12., 10., 10., 18., 12., 26., 12., 12., 12., 14.,
    ];
    w.extend([12., 13., 10., 11., 14., 13., 14., 13., 16., 13., 40.]);
    w.extend(std::iter::repeat_n(14., kinds.len() + 1));
    header(ws, &hdr, &w)?;
    let (plain, m2, m4) = (base(), money(), money4());
    let t = |x: &chrono::DateTime<chrono::Utc>| x.format("%Y-%m-%d %H:%M:%S").to_string();
    for (k, r) in rows.iter().enumerate() {
        let (row, i) = (k as u32 + 1, k as u32 + 2);
        let texts = [
            r.id.clone(),
            t(&r.start),
            r.end.as_ref().map(t).unwrap_or_default(),
            r.date.clone(),
        ];
        for (c, s) in texts.iter().enumerate() {
            ws.write_string_with_format(row, c as u16, s, &plain)?;
        }
        ws.write_number_with_format(row, 4, r.hour, &plain)?;
        for (c, s) in [
            (5, &r.coin),
            (6, &r.buy),
            (7, &r.sell),
            (9, &r.owner),
            (10, &r.status),
        ] {
            ws.write_string_with_format(row, c, s, &plain)?;
        }
        ws.write_formula_with_format(row, 8, Formula::new(format!("=G{i}&\" → \"&H{i}")), &plain)?;
        ws.write_number_with_format(row, 11, r.invested, &m2)?;
        for (c, v) in [
            (12, r.proceeds),
            (13, r.spent),
            (14, r.ret_fact),
            (18, r.plan),
        ] {
            ws.write_number_with_format(row, c, v, &m4)?;
        }
        if let Some(v) = r.fact {
            ws.write_number_with_format(row, 15, v, &m4)?;
        }
        if let Some(v) = r.estimate {
            ws.write_number_with_format(row, 16, v, &m4)?;
        }
        let pm = format!(
            "=IF(ISNUMBER(P{i}),IF(P{i}>0,\"плюс\",IF(P{i}<0,\"минус\",\"ноль\")),\"оценка\")"
        );
        ws.write_formula_with_format(row, 17, Formula::new(pm), &plain)?;
        if let Some((res, ret)) = r.plan_parts {
            ws.write_number_with_format(row, 19, res, &m4)?;
            ws.write_number_with_format(row, 20, ret, &m4)?;
        }
        let fs = [
            (21, format!("=S{i}+N(T{i})+N(U{i})")),
            (22, format!("=IF(ISNUMBER(P{i}),P{i}+O{i},\"\")")),
            (23, format!("=IF(ISNUMBER(P{i}),W{i}-V{i},\"\")")),
        ];
        for (c, f) in fs {
            ws.write_formula_with_format(row, c, Formula::new(f), &m4)?;
        }
        if let Some(s) = r.duration_s {
            ws.write_number_with_format(row, 24, s as f64, &plain)?;
        }
        ws.write_string_with_format(row, 25, &r.usdt_return, &plain)?;
        for (j, kind) in kinds.iter().enumerate() {
            let v = r.costs.get(kind).copied().unwrap_or(0.0);
            ws.write_number_with_format(row, 26 + j as u16, v, &m4)?;
        }
        ws.write_number_with_format(row, 26 + kinds.len() as u16, r.unsplit, &m4)?;
    }
    let last = hdr.len() as u16 - 1;
    ws.autofilter(0, 0, (rows.len() as u32).max(1), last)?;
    if !rows.is_empty() {
        red_green(ws, 1, 15, rows.len() as u32, 15)?;
    }
    Ok(())
}
