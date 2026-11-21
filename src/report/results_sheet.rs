//! Файл: report/results_sheet.rs
//! Что делает: решения 193, 257 — оформление отчётов: шапка (Arial, жирный белый на 1F4E78, высота 31.5, закрепление
//! первой строки), форматы чисел, красный / зелёный итог, примечание курсивом; ключ строки сводки. Сводный лист по
//! ключу — report/res_summary.rs.
//! Кто вызывает: report/res_summary.rs, res_deals.rs, res_days.rs, res_ledger.rs, res_total.rs, snap_sheet.rs;
//! missed_xlsx.rs, missed_cands.rs, missed_total.rs, cap_*.rs, sig_sheet.rs.
//! От чего зависит: rust_xlsxwriter.
//! Настройки: нет.

use rust_xlsxwriter::{
    Color, ConditionalFormatCell, ConditionalFormatCellRule, Format, FormatAlign, FormatPattern,
    Worksheet, XlsxError,
};

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
