//! Файл: report/results_sheet.rs
//! Что делает: решения 193, 257 — оформление отчётов (шапка Arial, жирный белый на 1F4E78, высота 31.5, закрепление
//! первой строки; форматы чисел; красный / зелёный итог; примечание курсивом) и сводный лист «Итогов сделок» по ключу
//! (биржа итога, направление, монета, час конца) формулами по листу «Сделки»: COUNTIFS / SUMIFS; «Лучшая / Худшая» —
//! формулой массива MAX(IF(…)) / MIN(IF(…)) (решение 257: MAXIFS / MINIFS нет в Excel до 2019 — у владельца #NAME?;
//! формула массива считается в Excel 2007+ и LibreOffice). Строка «Итого» — суммы, отношения — из сумм.
//! Каждая сделка — ровно у одного ключа, поэтому «Итого» любой сводки = листу «Итог».
//! Кто вызывает: report/results_xlsx.rs, report/res_days.rs, report/snap_sheet.rs; missed_xlsx.rs, missed_cands.rs,
//! missed_total.rs, cap_*.rs, sig_sheet.rs (шапка, форматы, примечание, цвет итога).
//! От чего зависит: rust_xlsxwriter, report/res_deals.rs (буквы колонок «Сделок», диапазоны).
//! Настройки: нет.

use rust_xlsxwriter::{
    Color, ConditionalFormatCell, ConditionalFormatCellRule, Format, FormatAlign, FormatPattern,
    Formula, Worksheet, XlsxError,
};

use super::res_deals::{C, rng};

/// Ключ строки сводки: текст (биржа, направление, монета) или число (час).
pub enum Key {
    Text(String),
    Num(u32),
}

/// Шрифт ячеек.
pub fn base() -> Format {
    Format::new().set_font_name("Arial")
}

/// Деньги 2 знака.
pub fn money() -> Format {
    base().set_num_format("#,##0.00;\\-#,##0.00;0.00")
}

/// Деньги 4 знака.
pub fn money4() -> Format {
    base().set_num_format("#,##0.0000;\\-#,##0.0000;0.0000")
}

/// Формат шапки: жирный белый на 1F4E78, по центру с переносом.
pub fn head() -> Format {
    base()
        .set_bold()
        .set_font_color(Color::White)
        .set_background_color(Color::RGB(0x1F4E78))
        .set_pattern(FormatPattern::Solid)
        .set_align(FormatAlign::Center)
        .set_align(FormatAlign::VerticalCenter)
        .set_text_wrap()
}

/// Шапка листа: заголовки, ширины, высота 31.5, закрепление первой строки. Ошибки: XlsxError.
pub fn header(ws: &mut Worksheet, hdr: &[&str], widths: &[f64]) -> Result<(), XlsxError> {
    let h = head();
    for (c, t) in hdr.iter().enumerate() {
        ws.write_string_with_format(0, c as u16, *t, &h)?;
        ws.set_column_width(c as u16, widths.get(c).copied().unwrap_or(14.))?;
    }
    ws.set_freeze_panes(1, 0)?;
    ws.set_row_height(0, 31.5)?;
    Ok(())
}

/// Итог красным (< 0) и зелёным (> 0) в диапазоне. Ошибки: XlsxError.
pub fn red_green(ws: &mut Worksheet, r1: u32, c1: u16, r2: u32, c2: u16) -> Result<(), XlsxError> {
    let red = ConditionalFormatCell::new()
        .set_rule(ConditionalFormatCellRule::LessThan(0))
        .set_format(base().set_font_color(Color::RGB(0xC00000)));
    let green = ConditionalFormatCell::new()
        .set_rule(ConditionalFormatCellRule::GreaterThan(0))
        .set_format(base().set_font_color(Color::RGB(0x007A33)));
    ws.add_conditional_format(r1, c1, r2, c2, &red)?;
    ws.add_conditional_format(r1, c1, r2, c2, &green)?;
    Ok(())
}

/// Примечание курсивом серым (595959) в столбце A. Ошибки: XlsxError.
pub fn italic_note(ws: &mut Worksheet, row: u32, t: &str) -> Result<(), XlsxError> {
    let f = base().set_italic().set_font_color(Color::RGB(0x595959));
    ws.write_string_with_format(row, 0, t, &f)?;
    Ok(())
}

/// Шапка сводки (первая колонка — у листа своя).
const COLS: [&str; 15] = [
    "",
    "Сделок",
    "Закрыто (итог — факт)",
    "Плюс",
    "Минус",
    "Винрейт (плюс / закрыто)",
    "Итог факт, USDT",
    "Оценка (не факт), USDT",
    "Факт + оценка, USDT",
    "План закрытых, USDT",
    "Факт − план (до запаса, без возврата USDT)",
    "Средний итог закрытой, USDT",
    "Вложено, USDT",
    "Лучшая (факт)",
    "Худшая (факт)",
];

/// Формат столбца сводки (1 — B … 14 — O); bold — строка «Итого».
fn fmt(col: u16, bold: bool) -> Format {
    let f = match col {
        5 => base().set_num_format("0.0%"),
        6..=11 | 13 | 14 => money4(),
        12 => money(),
        _ => base(),
    };
    if bold { f.set_bold() } else { f }
}

/// Лист сводки по ключу (колонка `key` листа «Сделки»). Принимает: лист, имя, заголовок первого столбца, ключи,
/// колонку ключа, последнюю строку «Сделок», примечания под таблицей. Ошибки: XlsxError.
#[allow(clippy::too_many_arguments)]
pub fn summary(
    ws: &mut Worksheet,
    title: &str,
    first: &str,
    keys: &[Key],
    key: &str,
    n: u32,
    notes: &[&str],
) -> Result<(), XlsxError> {
    ws.set_name(title)?;
    let mut hdr = COLS;
    hdr[0] = first;
    let w = [
        16., 9., 10., 8., 8., 11., 13., 13., 13., 13., 16., 13., 13., 12., 12.,
    ];
    header(ws, &hdr, &w)?;
    let r = |c: &str| rng(c, n);
    let (fact, est, plan, cmp, inv) =
        (r(C.fact), r(C.estimate), r(C.plan), r(C.cmp), r(C.invested));
    for (k, key_v) in keys.iter().enumerate() {
        let (row, j) = (k as u32 + 1, k as u32 + 2);
        match key_v {
            Key::Text(t) => ws.write_string_with_format(row, 0, t, &base())?,
            Key::Num(h) => ws.write_number_with_format(row, 0, *h, &base())?,
        };
        let c = format!("{},A{j}", r(key));
        let fs = [
            format!("=COUNTIFS({c})"),
            format!("=COUNTIFS({c},{fact},\"<>\")"),
            format!("=COUNTIFS({c},{fact},\">0\")"),
            format!("=COUNTIFS({c},{fact},\"<0\")"),
            format!("=IFERROR(D{j}/C{j},0)"),
            format!("=SUMIFS({fact},{c})"),
            format!("=SUMIFS({est},{c})"),
            format!("=G{j}+H{j}"),
            format!("=SUMIFS({plan},{c},{fact},\"<>\")"),
            format!("=SUMIFS({cmp},{c})"),
            format!("=IFERROR(G{j}/C{j},0)"),
            format!("=SUMIFS({inv},{c})"),
        ];
        for (i, f) in fs.iter().enumerate() {
            let col = i as u16 + 1;
            ws.write_formula_with_format(row, col, Formula::new(f.as_str()), &fmt(col, false))?;
        }
        // Решение 257: формула массива — совместима с Excel 2007+ и LibreOffice (вместо MAXIFS / MINIFS).
        let cond = format!("({}=A{j})*ISNUMBER({fact})", r(key));
        for (col, func) in [(13u16, "MAX"), (14u16, "MIN")] {
            let f = format!("=IF(C{j}=0,\"\",{func}(IF({cond},{fact})))");
            ws.write_array_formula_with_format(
                row,
                col,
                row,
                col,
                Formula::new(f),
                &fmt(col, false),
            )?;
        }
    }
    let tr = keys.len() as u32 + 1;
    total_row(ws, tr)?;
    red_green(ws, 1, 6, tr, 6)?;
    for (k, t) in notes.iter().enumerate() {
        italic_note(ws, tr + 2 + k as u32, t)?;
    }
    Ok(())
}

/// Строка «Итого» сводки в строке `tr` (0-based): суммы, отношения из сумм, лучшая / худшая по строкам.
/// Ошибки: XlsxError.
fn total_row(ws: &mut Worksheet, tr: u32) -> Result<(), XlsxError> {
    let t = tr + 1;
    let last = t - 1;
    ws.write_string_with_format(tr, 0, "Итого", &base().set_bold())?;
    if last < 2 {
        // Строк нет: диапазон «2:1» захватил бы саму строку «Итого» (циклическая ссылка) — нули без формул.
        for col in 1..13u16 {
            ws.write_number_with_format(tr, col, 0, &fmt(col, true))?;
        }
        return Ok(());
    }
    for col in 1..15u16 {
        let l = (b'A' + col as u8) as char;
        let f = match col {
            5 => format!("=IFERROR(D{t}/C{t},0)"),
            11 => format!("=IFERROR(G{t}/C{t},0)"),
            13 => format!("=IF(COUNT(N2:N{last})=0,\"\",MAX(N2:N{last}))"),
            14 => format!("=IF(COUNT(O2:O{last})=0,\"\",MIN(O2:O{last}))"),
            _ => format!("=SUM({l}2:{l}{last})"),
        };
        ws.write_formula_with_format(tr, col, Formula::new(f), &fmt(col, true))?;
    }
    Ok(())
}
