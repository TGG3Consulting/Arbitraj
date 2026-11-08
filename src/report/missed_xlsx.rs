//! Файл: report/missed_xlsx.rs
//! Что делает: решение 215 (утверждено Тиграном 07.11.2026) — книга Excel «Упущено» (arb_missed_<дата>.xlsx) по данным
//! экрана «Упущено из-за потолка» за период. Листы: «Итог» — строки «Исполненные сделки» и «Не исполненные
//! (кандидаты)» (записей, упёрлись в бюджет, план на нашем бюджете, факт, без потолка, упущено — суммы экрана,
//! core/uncapped_sum.rs Sum), ниже таблица «Потолок → прибыль» (потолок | сделки | кандидаты и строка «без потолка»);
//! «Сделки» — строка на сделку; «Кандидаты» — строка на эпизод; «Топ направлений» — монета, направление, сколько раз,
//! упущено всего, по убыванию; «Сделки — занижено» (решение 217) — сделки, посчитанные после нашей покупки (старые
//! записи), отдельно от остальных; у сделок столбец «Данные» — на каких данных посчитано. Решение 236: у сделок —
//! «Расчёт на вложенных», «Потолок (макс. прибыль без лимита): вложить», «На потолке заработали бы / потеряли бы,
//! расчёт», «Упущено против вложенного» (всё — одна модель по данным момента входа). Внизу «Сделок» и «Кандидатов» — строка «Итого» формулами SUM по столбцам денег и
//! счётчиков. Оформление — как «Итоги сделок» (report/results_sheet.rs): Arial, шапка белым по 1F4E78, высота 31.5,
//! закрепление первой строки, фильтр, форматы денег 2 и 4 знака, красный / зелёный факт, примечания курсивом.
//! Кто вызывает: report/mod.rs (missed).
//! От чего зависит: rust_xlsxwriter, report/missed_rows.rs, missed_total.rs (лист «Итог»), results_sheet.rs,
//! core/uncapped_report.rs.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_xlsxwriter::{Format, Formula, Workbook, Worksheet, XlsxError};

use super::missed_rows as mr;
use super::missed_total::total;
use super::results_sheet::{header, money, money4, red_green};
use crate::core::uncapped_report::MissedData;

/// Книга отчёта. Принимает: данные, подпись периода, время выгрузки. Возвращает: байты .xlsx. Ошибки: XlsxError.
pub fn workbook(data: &MissedData, period: &str, now: DateTime<Utc>) -> Result<Vec<u8>, XlsxError> {
    let mut wb = Workbook::new();
    total(wb.add_worksheet(), data, period, now)?;
    deals(wb.add_worksheet(), &mr::deals(data, false), "Сделки")?;
    candidates(wb.add_worksheet(), &mr::candidates(data))?;
    top(wb.add_worksheet(), &mr::top(data))?;
    // Решение 217: сделки, посчитанные после нашей покупки (занижено), — свой лист и своя строка «Итого».
    deals(
        wb.add_worksheet(),
        &mr::deals(data, true),
        "Сделки — занижено",
    )?;
    wb.save_to_buffer()
}

/// Шрифт ячеек (Arial, как «Итоги сделок»).
pub fn plain() -> Format {
    Format::new().set_font_name("Arial")
}

/// Жирный (строки «Итого»).
pub fn bold() -> Format {
    plain().set_bold()
}

/// Строка «Итого» формулами: SUM(<столбец>2:<столбец>N) по столбцам `cols` (0 — A). Принимает: лист, строку «Итого»
/// (0-based; данные — строки 1…row−1), столбцы с форматом. Ошибки: XlsxError.
fn sum_row(ws: &mut Worksheet, row: u32, cols: &[(u16, Format)]) -> Result<(), XlsxError> {
    ws.write_string_with_format(row, 0, "Итого", &bold())?;
    for (c, f) in cols {
        let l = (b'A' + *c as u8) as char;
        let fm = format!("=SUM({l}2:{l}{row})");
        ws.write_formula_with_format(row, *c, Formula::new(fm), &f.clone().set_bold())?;
    }
    Ok(())
}

/// Лист сделок («Сделки» или «Сделки — занижено», решение 217). Принимает: лист, строки, имя листа. Ошибки: XlsxError.
fn deals(ws: &mut Worksheet, rows: &[mr::DealRow], name: &str) -> Result<(), XlsxError> {
    ws.set_name(name)?;
    let hdr = [
        "Время (UTC)",
        "Монета",
        "Купить → продать",
        "Зашли на, USDT",
        "Что ограничило",
        "План, USDT",
        "Факт, USDT",
        "Расчёт на вложенных, USDT",
        "Потолок (макс. прибыль без лимита): вложить, USDT",
        "На потолке заработали бы (+) / потеряли бы (−), расчёт, USDT",
        "Упущено против вложенного, USDT",
        "Окно с (UTC)",
        "Данные",
    ];
    header(
        ws,
        &hdr,
        &[
            19., 14., 16., 12., 34., 11., 11., 14., 18., 20., 14., 36., 30.,
        ],
    )?;
    let (p, m, m4) = (plain(), money(), money4());
    for (k, r) in rows.iter().enumerate() {
        let row = k as u32 + 1;
        ws.write_string_with_format(row, 0, &r.time, &p)?;
        ws.write_string_with_format(row, 1, &r.coin, &p)?;
        ws.write_string_with_format(row, 2, &r.dir, &p)?;
        ws.write_number_with_format(row, 3, r.invested, &m)?;
        ws.write_string_with_format(row, 4, &r.limited, &p)?;
        ws.write_number_with_format(row, 5, r.plan, &m4)?;
        match r.fact {
            Some(f) => ws.write_number_with_format(row, 6, f, &m4)?,
            None => ws.write_blank(row, 6, &m4)?,
        };
        // Решение 236: расчёт той же моделью на вложенном объёме (по данным момента входа); нет точки — пусто.
        match r.at_invested {
            Some(x) => ws.write_number_with_format(row, 7, x, &m4)?,
            None => ws.write_blank(row, 7, &m4)?,
        };
        ws.write_number_with_format(row, 8, r.opt_invested, &m)?;
        ws.write_number_with_format(row, 9, r.opt_profit, &m4)?;
        ws.write_number_with_format(row, 10, r.missed, &m4)?;
        ws.write_string_with_format(row, 11, &r.window, &p)?;
        ws.write_string_with_format(row, 12, &r.basis, &p)?;
    }
    let tr = rows.len() as u32 + 1;
    ws.autofilter(0, 0, tr - 1, 12)?;
    red_green(ws, 1, 6, tr, 6)?;
    sum_row(
        ws,
        tr,
        &[
            (3, money()),
            (5, money4()),
            (6, money4()),
            (7, money4()),
            (8, money()),
            (9, money4()),
            (10, money4()),
        ],
    )
}

/// Лист «Кандидаты». Ошибки: XlsxError.
fn candidates(ws: &mut Worksheet, rows: &[mr::CandRow]) -> Result<(), XlsxError> {
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
    ];
    header(
        ws,
        &hdr,
        &[19., 19., 14., 16., 40., 10., 11., 12., 14., 14., 15., 12.],
    )?;
    let (p, m, m4) = (plain(), money(), money4());
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
    }
    let tr = rows.len() as u32 + 1;
    ws.autofilter(0, 0, tr - 1, 11)?;
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

/// Лист «Топ направлений». Ошибки: XlsxError.
fn top(ws: &mut Worksheet, rows: &[mr::TopRow]) -> Result<(), XlsxError> {
    ws.set_name("Топ направлений")?;
    header(
        ws,
        &[
            "Монета",
            "Направление",
            "Сколько раз",
            "Упущено всего, USDT",
        ],
        &[18., 18., 12., 16.],
    )?;
    let (p, m4) = (plain(), money4());
    for (k, r) in rows.iter().enumerate() {
        let row = k as u32 + 1;
        ws.write_string_with_format(row, 0, &r.coin, &p)?;
        ws.write_string_with_format(row, 1, &r.dir, &p)?;
        ws.write_number_with_format(row, 2, r.count as f64, &p)?;
        ws.write_number_with_format(row, 3, r.missed, &m4)?;
    }
    ws.autofilter(0, 0, rows.len() as u32, 3)?;
    Ok(())
}
