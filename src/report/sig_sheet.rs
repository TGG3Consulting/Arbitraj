//! Файл: report/sig_sheet.rs
//! Что делает: решение 255 — помощники листов книги «Сигналы и сделки»: объёмные строки части (все или доля k из n
//! одного сигнала), таблица с шапкой и продолжением на следующем листе после предела строк Excel, запись строк и чисел.
//! Кто вызывает: report/sig_xlsx.rs.
//! От чего зависит: rust_xlsxwriter, report/results_sheet.rs (header), sig_model.rs, sig_xlsx.rs (Part, SHEET_MAX_ROWS).
//! Настройки: нет.

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_xlsxwriter::{Workbook, Worksheet, XlsxError};

use super::results_sheet::header;
use super::sig_model::Sig;
use super::sig_xlsx::{Part, SHEET_MAX_ROWS};

/// Объёмные строки части: всех сигналов или доля k из n одного сигнала. Ошибок нет.
pub(super) fn details<'a, T>(part: &Part<'a>, f: impl Fn(&'a Sig) -> &'a Vec<T>) -> Vec<&'a T> {
    match part.cut {
        None => part.sigs.iter().flat_map(|s| f(s).iter()).collect(),
        Some((k, n)) => part
            .sigs
            .iter()
            .flat_map(|s| {
                let v = f(s);
                let (a, b) = (v.len() * k / n, v.len() * (k + 1) / n);
                v[a..b].iter()
            })
            .collect(),
    }
}

/// Таблица с шапкой; не влезла в лист — продолжение на листе «<имя> (2)» и т. д. Ошибки: XlsxError.
pub(super) fn table<T>(
    wb: &mut Workbook,
    name: &str,
    hdr: &[&str],
    widths: &[f64],
    rows: &[&T],
    write: impl Fn(&mut Worksheet, u32, &T) -> Result<(), XlsxError>,
) -> Result<(), XlsxError> {
    let per = SHEET_MAX_ROWS - 1;
    let chunks: Vec<&[&T]> = if rows.is_empty() {
        vec![&[]]
    } else {
        rows.chunks(per).collect()
    };
    for (i, chunk) in chunks.iter().enumerate() {
        let ws = wb.add_worksheet();
        ws.set_name(if i == 0 {
            name.to_string()
        } else {
            format!("{name} ({})", i + 1)
        })?;
        header(ws, hdr, widths)?;
        for (j, x) in chunk.iter().enumerate() {
            write(ws, j as u32 + 1, x)?;
        }
        ws.autofilter(0, 0, chunk.len() as u32, hdr.len() as u16 - 1)?;
    }
    Ok(())
}

/// Строки подряд с колонки 0. Ошибки: XlsxError.
pub(super) fn put(ws: &mut Worksheet, r: u32, v: &[&String]) -> Result<(), XlsxError> {
    for (c, t) in v.iter().enumerate() {
        ws.write_string(r, c as u16, t.as_str())?;
    }
    Ok(())
}

/// Числа подряд с колонки c0 (None — пустая ячейка). Ошибки: XlsxError.
pub(super) fn nums(
    ws: &mut Worksheet,
    r: u32,
    c0: u16,
    v: &[Option<Decimal>],
) -> Result<(), XlsxError> {
    for (i, x) in v.iter().enumerate() {
        if let Some(f) = x.and_then(|d| d.to_f64()) {
            ws.write_number(r, c0 + i as u16, f)?;
        }
    }
    Ok(())
}
