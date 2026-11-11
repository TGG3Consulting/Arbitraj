//! Файл: report/res_ledger.rs
//! Что делает: решение 257 — лист «Учёт дня» отчёта «Итоги сделок» (вынесено из res_days.rs — размер файла): строка на
//! начисление сделки в итог дня бота (report/day_rows.rs): день и время UTC, сделка, монета, направление, площадка итога,
//! часть («факт» / «оценка»), сумма, что это, источник («учёт сделки» / «восстановлено»). По нему считает «По дням».
//! Кто вызывает: report/results_xlsx.rs.
//! От чего зависит: rust_xlsxwriter, report/day_rows.rs, report/res_days.rs (FACT, ESTIMATE, RESTORED),
//! report/results_sheet.rs.
//! Настройки: нет.

use rust_xlsxwriter::{Worksheet, XlsxError};

use super::day_rows::DayRow;
use super::res_days::{ESTIMATE, FACT, RESTORED};
use super::results_sheet::{base, header, money4};

/// Лист «Учёт дня». Ошибки: XlsxError.
pub fn ledger(ws: &mut Worksheet, rows: &[DayRow]) -> Result<(), XlsxError> {
    ws.set_name("Учёт дня")?;
    let hdr = [
        "День (UTC)",
        "Время (UTC)",
        "Сделка (id)",
        "Монета",
        "Направление",
        "Площадка итога",
        "Часть итога дня",
        "Сумма, USDT",
        "Что",
        "Источник",
    ];
    header(
        ws,
        &hdr,
        &[12., 10., 18., 12., 18., 12., 10., 12., 50., 16.],
    )?;
    let p = base();
    for (k, r) in rows.iter().enumerate() {
        let row = k as u32 + 1;
        let part = if r.fact { FACT } else { ESTIMATE };
        let src = if r.booked {
            "учёт сделки"
        } else {
            RESTORED
        };
        let texts = [
            r.date.to_string(),
            r.at.format("%H:%M:%S").to_string(),
            r.deal.clone(),
            r.coin.clone(),
            r.direction.clone(),
            r.owner.clone(),
            part.to_string(),
        ];
        for (c, t) in texts.iter().enumerate() {
            ws.write_string_with_format(row, c as u16, t, &p)?;
        }
        ws.write_number_with_format(row, 7, r.amount, &money4())?;
        ws.write_string_with_format(row, 8, &r.what, &p)?;
        ws.write_string_with_format(row, 9, src, &p)?;
    }
    ws.autofilter(0, 0, (rows.len() as u32).max(1), 9)?;
    Ok(())
}
