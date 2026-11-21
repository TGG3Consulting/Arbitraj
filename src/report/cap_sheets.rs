//! Файл: report/cap_sheets.rs
//! Что делает: решение 256 — общее листов отчёта «Сделки, лимиты и капитал» (имена листов, буквы столбцов листа
//! «Сделки», диапазоны, время числом Excel) и лист «Упущено из-за потолка» (строка «день × направление»). Лист
//! «Сделки» — report/cap_deals.rs. Оформление — как у «Итогов сделок» (report/results_sheet.rs: Arial, шапка белым по
//! 1F4E78, закрепление первой строки), деньги — 2 знака, фильтр по шапке.
//! Время пишется числом Excel (система дат 1900: сутки от 30.12.1899; 25569 — 01.01.1970, ECMA-376 часть 1, 18.17.4.1).
//! Кто вызывает: report/cap_xlsx.rs, cap_deals.rs, cap_summary.rs, cap_capital.rs.
//! От чего зависит: rust_xlsxwriter, report/results_sheet.rs (header, money), cap_episodes.rs.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_xlsxwriter::{Format, Worksheet, XlsxError};

use super::cap_episodes::{DayRow, LIMITS};
use super::results_sheet::{header, money};

/// Имена листов (на них ссылаются формулы листа «Итоги»).
pub const DEALS: &str = "Сделки";
pub const MISSED: &str = "Упущено из-за потолка";

/// Столбцы листа «Сделки» (буквы — для формул; порядок — report/cap_deals.rs HDR).
pub mod d {
    pub const DIR: &str = "F";
    pub const BOUGHT: &str = "G";
    pub const START: &str = "I";
    pub const END: &str = "J";
    pub const DUR: &str = "K";
    pub const INVESTED: &str = "M";
    pub const SPENT: &str = "O";
    pub const RET_COST: &str = "P";
    pub const PLAN: &str = "Q";
    pub const FACT: &str = "R";
    pub const ESTIMATE: &str = "S";
    pub const IN_TOTAL: &str = "T";
    pub const PLACE: &str = "W";
    pub const FREE_AT: &str = "X";
    pub const COUNTED: &str = "Y";
    pub const BUSY: &str = "AA";
    pub const OPEN_N: &str = "AB";
    pub const BUSY_ALL: &str = "AC";
}

/// «Да» в столбце «Покупка состоялась».
pub const YES: &str = "Да";

/// Буквы столбца по номеру с нуля (A, …, Z, AA, …). Ошибок нет.
pub fn col(i: u16) -> String {
    let mut n = u32::from(i) + 1;
    let mut s = Vec::new();
    while n > 0 {
        let r = ((n - 1) % 26) as u8;
        s.push((b'A' + r) as char);
        n = (n - 1) / 26;
    }
    s.iter().rev().collect()
}

/// Диапазон столбца листа для формул: 'Лист'!$C$2:$C$N. Ошибок нет.
pub fn rng(sheet: &str, c: &str, last: u32) -> String {
    format!("'{sheet}'!${c}$2:${c}${last}")
}

/// Время в число Excel (см. шапку), округлённое до 10 знаков после запятой (≈ 9 мкс): у такого числа не больше 15
/// значащих цифр, поэтому условие COUNTIFS "<="&I2 (Excel и LibreOffice переводят число в текст с 15 значащими цифрами)
/// даёт ровно то же число и сделка находит саму себя. Ошибок нет.
pub fn serial(t: DateTime<Utc>) -> f64 {
    let x = t.timestamp_millis() as f64 / 86_400_000.0 + 25569.0;
    (x * 1e10).round() / 1e10
}

/// Формат времени.
pub fn time_fmt() -> Format {
    Format::new()
        .set_font_name("Arial")
        .set_num_format("dd.mm.yy hh:mm:ss")
}

/// Формат текста и целых.
pub fn plain() -> Format {
    Format::new().set_font_name("Arial")
}

/// Последняя строка данных для диапазонов (1-я — шапка; пустой лист — диапазон из одной пустой строки). Ошибок нет.
pub fn last_row(n: usize) -> u32 {
    (n as u32 + 1).max(2)
}

/// Подпись потолка: «100», «1000». Ошибок нет.
pub fn cap_text(c: Decimal) -> String {
    c.normalize().to_string()
}

/// Заголовки листа «Упущено из-за потолка» по потолкам. Ошибок нет.
pub fn missed_header(ladder: &[Decimal]) -> Vec<String> {
    let mut h: Vec<String> = [
        "День (UTC)",
        "Направление",
        "Эпизодов",
        "Эпизодов со сделкой",
        "Прибыль на нашем бюджете (модель)",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for c in ladder {
        h.push(format!("Прибыль при лимите {} USDT (модель)", cap_text(*c)));
    }
    for l in LIMITS {
        h.push(format!("Сколько раз ограничил: {l}"));
    }
    h
}

/// Лист «Упущено из-за потолка». Принимает: лист, строки, потолки. Ошибки: XlsxError.
pub fn missed(ws: &mut Worksheet, rows: &[DayRow], ladder: &[Decimal]) -> Result<(), XlsxError> {
    ws.set_name(MISSED)?;
    let h = missed_header(ladder);
    let hdr: Vec<&str> = h.iter().map(String::as_str).collect();
    let mut widths = vec![12., 16., 10., 11., 16.];
    widths.extend(ladder.iter().map(|_| 16.));
    widths.extend(LIMITS.iter().map(|_| 13.));
    header(ws, &hdr, &widths)?;
    let (p, m) = (plain(), money());
    let day = plain().set_num_format("dd.mm.yy");
    for (i, x) in rows.iter().enumerate() {
        let row = i as u32 + 1;
        let midnight = x.day.and_hms_opt(0, 0, 0).map(|t| t.and_utc());
        if let Some(t) = midnight {
            ws.write_number_with_format(row, 0, serial(t), &day)?;
        }
        ws.write_string_with_format(row, 1, &x.dir, &p)?;
        ws.write_number_with_format(row, 2, x.episodes as f64, &p)?;
        ws.write_number_with_format(row, 3, x.with_deal as f64, &p)?;
        ws.write_number_with_format(row, 4, x.at_budget, &m)?;
        for (k, v) in x.caps.iter().enumerate() {
            if let Some(v) = v {
                ws.write_number_with_format(row, 5 + k as u16, *v, &m)?;
            }
        }
        let base = 5 + ladder.len() as u16;
        for (k, v) in x.limits.iter().enumerate() {
            ws.write_number_with_format(row, base + k as u16, *v as f64, &p)?;
        }
    }
    ws.autofilter(0, 0, rows.len() as u32, (hdr.len() - 1) as u16)?;
    Ok(())
}
