//! Файл: report/cap_sheets.rs
//! Что делает: решение 256 — листы данных отчёта «Сделки, лимиты и капитал»: «Сделки» (строка на сделку, столбец
//! «Одновременно открыто в момент начала» и «Длительность, с» — формулами) и «Упущено из-за потолка» (строка «день ×
//! направление»). Оформление — как у «Итогов сделок» (report/results_sheet.rs: Arial, шапка белым по 1F4E78, закрепление
//! первой строки), деньги — 2 знака, время — дд.мм.гг чч:мм:сс (UTC), фильтр по шапке.
//! «Одновременно открыто в момент начала» (только у сделок с покупкой): сделки того же направления с покупкой, у которых
//! начало ≤ начала этой и конец ≥ начала этой или конца нет (сделка ещё открыта), — включая её саму:
//! COUNTIFS(направление, покупка «Да», начало "<="&I, конец ">="&I) + COUNTIFS(…, конец "").
//! Время пишется числом Excel (система дат 1900: сутки от 30.12.1899; 25569 — 01.01.1970, ECMA-376 часть 1, 18.17.4.1).
//! Кто вызывает: report/cap_xlsx.rs.
//! От чего зависит: rust_xlsxwriter, report/results_sheet.rs (header, money), cap_rows.rs, cap_episodes.rs.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_xlsxwriter::{Format, Formula, Worksheet, XlsxError};

use super::cap_episodes::{DayRow, LIMITS};
use super::cap_rows::DealRow;
use super::results_sheet::{header, money};

/// Имена листов (на них ссылаются формулы листа «Итоги»).
pub const DEALS: &str = "Сделки";
pub const MISSED: &str = "Упущено из-за потолка";

/// Столбцы листа «Сделки» (буквы — для формул «Итогов»).
pub mod d {
    pub const DIR: char = 'F';
    pub const BOUGHT: char = 'G';
    pub const START: char = 'I';
    pub const END: char = 'J';
    pub const DUR: char = 'K';
    pub const PLAN: char = 'N';
    pub const FACT: char = 'O';
    pub const CONC: char = 'Q';
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
fn time_fmt() -> Format {
    Format::new()
        .set_font_name("Arial")
        .set_num_format("dd.mm.yy hh:mm:ss")
}

/// Формат текста и целых.
fn plain() -> Format {
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

/// Лист «Сделки». Принимает: лист, строки. Ошибки: XlsxError.
pub fn deals(ws: &mut Worksheet, rows: &[DealRow]) -> Result<(), XlsxError> {
    ws.set_name(DEALS)?;
    let hdr = [
        "№ сделки",
        "Монета",
        "Контракт",
        "Биржа покупки",
        "Биржа продажи",
        "Направление",
        "Покупка состоялась",
        "Статус",
        "Начало (UTC)",
        "Конец (UTC)",
        "Длительность, с",
        "Вложено, USDT",
        "Получено, USDT",
        "Прибыль по плану, USDT",
        "Итог по факту, USDT",
        "Возврат USDT (состояние)",
        "Одновременно открыто в момент начала (то же направление, с покупкой)",
    ];
    let widths = [
        26., 12., 44., 13., 13., 16., 11., 18., 18., 18., 12., 13., 13., 14., 14., 36., 22.,
    ];
    header(ws, &hdr, &widths)?;
    let (p, m, t) = (plain(), money(), time_fmt());
    let last = last_row(rows.len());
    let r = |c: char| rng(DEALS, &c.to_string(), last);
    for (i, x) in rows.iter().enumerate() {
        let row = i as u32 + 1;
        let n = row + 1;
        let texts = [&x.id, &x.coin, &x.contract, &x.buy, &x.sell, &x.dir];
        for (c, s) in texts.iter().enumerate() {
            ws.write_string_with_format(row, c as u16, s.as_str(), &p)?;
        }
        ws.write_string_with_format(row, 6, if x.bought { YES } else { "Нет" }, &p)?;
        ws.write_string_with_format(row, 7, &x.status, &p)?;
        ws.write_number_with_format(row, 8, serial(x.start), &t)?;
        if let Some(e) = x.end {
            ws.write_number_with_format(row, 9, serial(e), &t)?;
            let f = format!("=ROUND(({e}{n}-{s}{n})*86400,0)", e = d::END, s = d::START);
            ws.write_formula_with_format(row, 10, Formula::new(f), &p)?;
        }
        ws.write_number_with_format(row, 11, x.invested, &m)?;
        ws.write_number_with_format(row, 12, x.proceeds, &m)?;
        ws.write_number_with_format(row, 13, x.plan, &m)?;
        if let Some(v) = x.fact {
            ws.write_number_with_format(row, 14, v, &m)?;
        }
        ws.write_string_with_format(row, 15, &x.ret, &p)?;
        if x.bought {
            let same = format!(
                "{},{dir}{n},{},\"{YES}\",{},\"<=\"&{st}{n}",
                r(d::DIR),
                r(d::BOUGHT),
                r(d::START),
                dir = d::DIR,
                st = d::START
            );
            let f = format!(
                "=COUNTIFS({same},{e},\">=\"&{st}{n})+COUNTIFS({same},{e},\"\")",
                e = r(d::END),
                st = d::START
            );
            ws.write_formula_with_format(row, 16, Formula::new(f), &p)?;
        }
    }
    ws.autofilter(0, 0, rows.len() as u32, (hdr.len() - 1) as u16)?;
    Ok(())
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
