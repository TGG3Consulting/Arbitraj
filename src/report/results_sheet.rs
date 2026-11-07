//! Файл: report/results_sheet.rs
//! Что делает: решение 193 — оформление и сводные листы отчёта «Итоги сделок» 1 в 1 как arb_results_2026-11-05.xlsx
//! (xl/build.py): шапка (Arial, жирный белый на 1F4E78, по центру с переносом, высота 31.5, закрепление первой строки),
//! ширины столбцов, форматы чисел, красный / зелёный итог (C00000 / 007A33), сводка по ключу (COUNTIFS / SUMIFS /
//! _xlfn.MAXIFS / _xlfn.MINIFS по листу «Сделки», строка «Итого» жирным), сводка по биржам (сделка считается и у биржи
//! покупки, и у биржи продажи; строки не складываются — вместо «Итого» примечание).
//! Кто вызывает: report/results_xlsx.rs, report/missed_xlsx.rs (решение 215: шапка, форматы, примечание, цвет итога).
//! От чего зависит: rust_xlsxwriter.
//! Настройки: нет.

use rust_xlsxwriter::{
    Color, ConditionalFormatCell, ConditionalFormatCellRule, Format, FormatAlign, FormatPattern,
    Formula, Worksheet, XlsxError,
};

use super::results_xlsx::rng;

/// Ключ строки сводки: текст (день, биржа, направление, монета) или число (час).
pub enum Key {
    Text(String),
    Num(u32),
}

/// Шрифт ячеек.
fn base() -> Format {
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

/// Формат шапки: жирный белый на 1F4E78, по центру с переносом (решение 215: и шапки таблиц внутри листа «Упущено»).
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
    let f = Format::new()
        .set_font_name("Arial")
        .set_italic()
        .set_font_color(Color::RGB(0x595959));
    ws.write_string_with_format(row, 0, t, &f)?;
    Ok(())
}

/// Формат столбца сводки (0 — A … 11 — L) в строке; bold — строка «Итого».
fn fmt(col: u16, bold: bool) -> Format {
    let f = match col {
        4 => base().set_num_format("0.0%"),
        5 | 6 | 7 | 8 | 10 | 11 => money(),
        9 => base().set_num_format("#,##0"),
        _ => base(),
    };
    if bold { f.set_bold() } else { f }
}

/// Шапка сводки.
const COLS: [&str; 12] = [
    "",
    "Сделок",
    "Плюс",
    "Минус",
    "Винрейт",
    "Итог факт, USDT",
    "План, USDT",
    "Факт − план",
    "Средний итог",
    "Вложено всего, USDT",
    "Лучшая",
    "Худшая",
];

/// Лист сводки по ключу (столбец `col` листа «Сделки»). Принимает: лист, имя, заголовок первого столбца, ключи,
/// столбец ключа, последнюю строку «Сделок», примечание под таблицей. Ошибки: XlsxError.
pub fn summary(
    ws: &mut Worksheet,
    title: &str,
    first: &str,
    keys: &[Key],
    col: char,
    n: u32,
    note: Option<&str>,
) -> Result<(), XlsxError> {
    ws.set_name(title)?;
    let r = |c| rng(c, n);
    let crit = |j: u32| format!("{},A{j}", r(col));
    let rows: Vec<Vec<String>> = (0..keys.len() as u32)
        .map(|k| {
            let j = k + 2;
            let c = crit(j);
            vec![
                format!("=COUNTIFS({c})"),
                format!("=COUNTIFS({c},{},\">0\")", r('J')),
                format!("=COUNTIFS({c},{},\"<0\")", r('J')),
                format!("=IFERROR(C{j}/B{j},0)"),
                format!("=SUMIFS({},{c})", r('J')),
                format!("=SUMIFS({},{c})", r('I')),
                format!("=F{j}-G{j}"),
                format!("=IFERROR(F{j}/B{j},0)"),
                format!("=SUMIFS({},{c})", r('H')),
                format!("=_xlfn.MAXIFS({},{c})", r('J')),
                format!("=_xlfn.MINIFS({},{c})", r('J')),
            ]
        })
        .collect();
    table(ws, first, keys, &rows, false)?;
    if let Some(t) = note {
        italic_note(ws, keys.len() as u32 + 3, t)?;
    }
    Ok(())
}

/// Таблица сводки: шапка, строки (ключ + 11 формул), строка «Итого» (venues — без неё, вместо — примечание),
/// форматы и красный / зелёный итог факт. Ошибки: XlsxError.
fn table(
    ws: &mut Worksheet,
    first: &str,
    keys: &[Key],
    rows: &[Vec<String>],
    venues: bool,
) -> Result<(), XlsxError> {
    let mut hdr = COLS;
    hdr[0] = first;
    header(
        ws,
        &hdr,
        &[16., 9., 8., 8., 10., 15., 12., 12., 12., 16., 11., 11.],
    )?;
    for (k, (key, fs)) in keys.iter().zip(rows).enumerate() {
        let row = k as u32 + 1;
        match key {
            Key::Text(t) => ws.write_string_with_format(row, 0, t, &fmt(0, false))?,
            Key::Num(h) => ws.write_number_with_format(row, 0, *h, &fmt(0, false))?,
        };
        for (c, f) in fs.iter().enumerate() {
            let col = c as u16 + 1;
            ws.write_formula_with_format(row, col, Formula::new(f.as_str()), &fmt(col, false))?;
        }
    }
    let tr = keys.len() as u32 + 1;
    let t = tr + 1;
    if venues {
        for col in 1..12u16 {
            ws.write_blank(tr, col, &fmt(col, true))?;
        }
        italic_note(
            ws,
            tr,
            "(сделка биржа ↔ биржа учтена у обеих бирж — строки не складываются)",
        )?;
    } else {
        ws.write_string_with_format(tr, 0, "Итого", &fmt(0, true))?;
        for col in 1..12u16 {
            let l = (b'A' + col as u8) as char;
            let f = match col {
                1 | 2 | 3 | 5 | 6 | 7 | 9 => format!("=SUM({l}2:{l}{})", t - 1),
                4 => format!("=IFERROR(C{t}/B{t},0)"),
                8 => format!("=IFERROR(F{t}/B{t},0)"),
                _ => String::new(),
            };
            if f.is_empty() {
                ws.write_blank(tr, col, &fmt(col, true))?;
            } else {
                ws.write_formula_with_format(tr, col, Formula::new(f), &fmt(col, true))?;
            }
        }
    }
    red_green(ws, 1, 5, tr, 5)?;
    Ok(())
}

/// Лист «По биржам»: сделка учитывается у биржи покупки и у биржи продажи. Ошибки: XlsxError.
pub fn summary_venues(ws: &mut Worksheet, venues: &[String], n: u32) -> Result<(), XlsxError> {
    ws.set_name("По биржам")?;
    let r = |c| rng(c, n);
    let rows: Vec<Vec<String>> = (0..venues.len() as u32)
        .map(|k| {
            let j = k + 2;
            let (b, s, jj) = (format!("{},A{j}", r('E')), format!("{},A{j}", r('F')), r('J'));
            let (i, h) = (r('I'), r('H'));
            vec![
                format!("=COUNTIFS({b})+COUNTIFS({s})"),
                format!("=COUNTIFS({b},{jj},\">0\")+COUNTIFS({s},{jj},\">0\")"),
                format!("=COUNTIFS({b},{jj},\"<0\")+COUNTIFS({s},{jj},\"<0\")"),
                format!("=IFERROR(C{j}/B{j},0)"),
                format!("=SUMIFS({jj},{b})+SUMIFS({jj},{s})"),
                format!("=SUMIFS({i},{b})+SUMIFS({i},{s})"),
                format!("=F{j}-G{j}"),
                format!("=IFERROR(F{j}/B{j},0)"),
                format!("=SUMIFS({h},{b})+SUMIFS({h},{s})"),
                format!(
                    "=IF(COUNTIFS({b})=0,_xlfn.MAXIFS({jj},{s}),IF(COUNTIFS({s})=0,_xlfn.MAXIFS({jj},{b}),MAX(_xlfn.MAXIFS({jj},{b}),_xlfn.MAXIFS({jj},{s}))))"
                ),
                format!(
                    "=IF(COUNTIFS({b})=0,_xlfn.MINIFS({jj},{s}),IF(COUNTIFS({s})=0,_xlfn.MINIFS({jj},{b}),MIN(_xlfn.MINIFS({jj},{b}),_xlfn.MINIFS({jj},{s}))))"
                ),
            ]
        })
        .collect();
    let keys: Vec<Key> = venues.iter().cloned().map(Key::Text).collect();
    table(ws, "Биржа", &keys, &rows, true)
}
