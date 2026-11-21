//! Файл: report/missed_cands.rs
//! Что делает: лист «Кандидаты» книги «Упущено» (решение 215): строка на эпизод — начало и конец эпизода, монета,
//! направление, вердикт, расчётов, упёрлись в бюджет, наш бюджет, без потолка вложить → прибыль, минимум до плюса,
//! упущено; решение 245 — окно максимального объёма (самое длинное в эпизоде): начало, конец, длительность (точно / не
//! меньше), вложить и прибыль на начале и в пике, окон в эпизоде, медиана и p75 длительности сделки пары (или
//! биржи-источника), по чему посчитано, «успели бы». Внизу — «Итого» формулами SUM (как было: F, G, I, J, L).
//! Перенесён из report/missed_xlsx.rs без изменения прежних столбцов (A–L).
//! Кто вызывает: report/missed_xlsx.rs (workbook).
//! От чего зависит: rust_xlsxwriter, report/missed_rows.rs (CandRow), missed_maxwin.rs, missed_xlsx.rs (plain, sum_row),
//! results_sheet.rs (header, money, money4).
//! Настройки: нет.

use rust_xlsxwriter::{Format, Worksheet, XlsxError};

use super::missed_rows as mr;
use super::missed_xlsx::{plain, sum_row};
use super::results_sheet::{header, money, money4};

/// Число или пусто. Ошибки: XlsxError.
fn opt(
    ws: &mut Worksheet,
    row: u32,
    col: u16,
    v: Option<f64>,
    f: &Format,
) -> Result<(), XlsxError> {
    match v {
        Some(x) => ws.write_number_with_format(row, col, x, f)?,
        None => ws.write_blank(row, col, f)?,
    };
    Ok(())
}

/// Лист «Кандидаты». Ошибки: XlsxError.
pub fn candidates(ws: &mut Worksheet, rows: &[mr::CandRow]) -> Result<(), XlsxError> {
    ws.set_name("Кандидаты")?;
    let hdr = [
        "Начало (UTC)",
        "Конец (UTC)",
        "Монета",
        "Купить → продать",
        "Вердикт / причина",
        "Расчётов",
        "Упёрлись в бюджет",
        "Наш бюджет, USDT",
        "Без потолка: вложить, USDT",
        "Без потолка: прибыль, USDT",
        "Минимум до плюса: вложить, USDT",
        "Упущено, USDT",
        "Окно макс. объёма: начало (UTC)",
        "Окно макс. объёма: конец (UTC)",
        "Окно макс. объёма: длительность, с",
        "Длительность",
        "На начале: вложить, USDT",
        "На начале: прибыль, USDT",
        "Пик: вложить, USDT",
        "Пик: прибыль, USDT",
        "Окон в эпизоде",
        "Сделка: медиана, с",
        "Сделка: p75, с",
        "Длительность сделки по",
        "Успели бы",
    ];
    header(
        ws,
        &hdr,
        &[
            19., 19., 14., 16., 40., 10., 11., 12., 14., 14., 15., 12., 19., 30., 12., 22., 14.,
            14., 14., 14., 10., 11., 11., 30., 40.,
        ],
    )?;
    let (p, m, m4) = (plain(), money(), money4());
    let s3 = Format::new().set_font_name("Arial").set_num_format("0.000");
    for (k, r) in rows.iter().enumerate() {
        let row = k as u32 + 1;
        ws.write_string_with_format(row, 0, &r.start, &p)?;
        ws.write_string_with_format(row, 1, &r.end, &p)?;
        ws.write_string_with_format(row, 2, &r.coin, &p)?;
        ws.write_string_with_format(row, 3, &r.dir, &p)?;
        ws.write_string_with_format(row, 4, &r.verdict, &p)?;
        ws.write_number_with_format(row, 5, r.evals as f64, &p)?;
        ws.write_number_with_format(row, 6, r.capped as f64, &p)?;
        ws.write_number_with_format(row, 7, r.budget, &m)?;
        ws.write_number_with_format(row, 8, r.opt_invested, &m)?;
        ws.write_number_with_format(row, 9, r.opt_profit, &m4)?;
        match r.min_plus {
            Some(x) => ws.write_number_with_format(row, 10, x, &m)?,
            None => ws.write_string_with_format(row, 10, "не становится плюсовой", &p)?,
        };
        ws.write_number_with_format(row, 11, r.missed, &m4)?;
        // Решение 245: окно максимального объёма и «успели бы».
        let w = &r.maxwin;
        ws.write_string_with_format(row, 12, &w.start, &p)?;
        ws.write_string_with_format(row, 13, &w.end, &p)?;
        opt(ws, row, 14, w.secs, &s3)?;
        ws.write_string_with_format(row, 15, &w.kind, &p)?;
        opt(ws, row, 16, w.start_invested, &m)?;
        opt(ws, row, 17, w.start_profit, &m4)?;
        opt(ws, row, 18, w.peak_invested, &m)?;
        opt(ws, row, 19, w.peak_profit, &m4)?;
        opt(ws, row, 20, w.n, &p)?;
        opt(ws, row, 21, w.median_s, &s3)?;
        opt(ws, row, 22, w.p75_s, &s3)?;
        ws.write_string_with_format(row, 23, &w.deal_by, &p)?;
        ws.write_string_with_format(row, 24, &w.would, &p)?;
    }
    let tr = rows.len() as u32 + 1;
    ws.autofilter(0, 0, tr - 1, 24)?;
    sum_row(
        ws,
        tr,
        &[
            (5, plain()),
            (6, plain()),
            (8, money()),
            (9, money4()),
            (11, money4()),
        ],
    )
}
