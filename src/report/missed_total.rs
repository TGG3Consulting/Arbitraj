//! Файл: report/missed_total.rs
//! Что делает: решение 215 — лист «Итог» книги «Упущено»: строки «Исполненные сделки» и «Не исполненные (кандидаты)»
//! (записей, упёрлись в бюджет, план на нашем бюджете, факт, без потолка, упущено — суммы экрана «Упущено из-за
//! потолка», core/uncapped_sum.rs Sum), ниже таблица «Потолок → прибыль» (uncapped.ladder_usdt: сделки | кандидаты и
//! строка «без потолка»), примечания: период, время выгрузки, что такое факт и упущено, причины (нет таблицы потолков,
//! скрытые старые записи, расчёт не идёт). Решение 217: строка «Сделки: посчитано после покупки — занижено» (итог
//! deals_old) — отдельно, если такие записи есть.
//! Кто вызывает: report/missed_xlsx.rs.
//! От чего зависит: rust_xlsxwriter, report/results_sheet.rs, missed_rows.rs (num), missed_xlsx.rs (plain, bold).
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_xlsxwriter::{Worksheet, XlsxError};

use super::missed_rows::num;
use super::missed_xlsx::{bold, plain};
use super::results_sheet::{head, header, italic_note, money, money4, red_green};
use crate::core::uncapped_report::MissedData;
use crate::core::uncapped_sum::Sum;

/// Строка группы на «Итоге». fact — писать ли факт (у кандидатов факта нет). Ошибки: XlsxError.
fn group(ws: &mut Worksheet, row: u32, title: &str, s: &Sum, fact: bool) -> Result<(), XlsxError> {
    let m4 = money4();
    ws.write_string_with_format(row, 0, title, &plain())?;
    ws.write_number_with_format(row, 1, s.count as f64, &plain())?;
    ws.write_number_with_format(row, 2, s.bound as f64, &plain())?;
    ws.write_number_with_format(row, 3, num(s.battle, 4), &m4)?;
    if fact {
        ws.write_number_with_format(row, 4, num(s.fact, 4), &m4)?;
    } else {
        ws.write_string_with_format(row, 4, "—", &plain())?;
    }
    ws.write_number_with_format(row, 5, num(s.potential, 4), &m4.clone().set_bold())?;
    ws.write_number_with_format(row, 6, num(s.missed, 4), &m4.set_bold())?;
    Ok(())
}

/// Лист «Итог». Принимает: лист, данные, подпись периода, время выгрузки. Ошибки: XlsxError.
pub fn total(
    ws: &mut Worksheet,
    data: &MissedData,
    period: &str,
    now: DateTime<Utc>,
) -> Result<(), XlsxError> {
    ws.set_name("Итог")?;
    let hdr = [
        "",
        "Записей",
        "Упёрлись в бюджет",
        "План на нашем бюджете, USDT",
        "Факт, USDT",
        "Без потолка, USDT",
        "Упущено, USDT",
    ];
    header(ws, &hdr, &[34., 10., 13., 16., 13., 14., 14.])?;
    let t = &data.totals;
    group(ws, 1, "Исполненные сделки", &t.deals, true)?;
    group(ws, 2, "Не исполненные (кандидаты)", &t.cands, false)?;
    red_green(ws, 1, 4, 1, 4)?;
    // Решение 217: сделки, посчитанные после нашей покупки (занижено), — своей строкой, в суммы выше не входят.
    if t.deals_old.count > 0 {
        group(
            ws,
            3,
            "Сделки: посчитано после покупки — занижено",
            &t.deals_old,
            true,
        )?;
        red_green(ws, 3, 4, 3, 4)?;
    }
    let h = 4u32;
    for (c, x) in ["Потолок, USDT", "Сделки, USDT", "Кандидаты, USDT"]
        .iter()
        .enumerate()
    {
        ws.write_string_with_format(h, c as u16, *x, &head())?;
    }
    ws.set_row_height(h, 31.5)?;
    let m4 = money4();
    let mut row = h + 1;
    for (cap, s) in &t.ladder {
        ws.write_number_with_format(row, 0, num(*cap, 2), &money())?;
        ws.write_number_with_format(row, 1, num(s[0], 4), &m4)?;
        ws.write_number_with_format(row, 2, num(s[1], 4), &m4)?;
        row += 1;
    }
    ws.write_string_with_format(row, 0, "без потолка", &bold())?;
    ws.write_number_with_format(row, 1, num(t.deals.potential, 4), &m4.clone().set_bold())?;
    ws.write_number_with_format(row, 2, num(t.cands.potential, 4), &m4.set_bold())?;
    let mut notes = vec![
        format!("Период: {period}."),
        format!(
            "Источник: записи расчёта «без потолка» (экран «Упущено из-за потолка»), выгрузка {} UTC.",
            now.format("%d.%m.%Y ~%H:%M")
        ),
        format!(
            "Факт — итог сделок из учёта (с итогом {} из {}). «Взяли бы» при потолке — лучший объём со списанием не больше потолка, если проходит пороги.",
            t.deals.fact_count, t.deals.count
        ),
        "Упущено: сделка — прибыль на потолке минус прибыль на вложенном объёме, одна модель по данным момента входа (решение 237); кандидат «прошёл» — без потолка минус на нашем бюджете; отказ — прибыль без потолка, если проходит пороги.".into(),
        "Потолок — сколько можно было вложить с максимальной прибылью без лимита; «на потолке заработали бы / потеряли бы» — расчёт модели, не факт (продажи на этом объёме не было).".into(),
    ];
    if let Some(e) = &data.ladder_error {
        notes.push(format!("Таблицы потолков нет: {e}"));
    }
    if t.hidden > 0 {
        notes.push(format!(
            "Старых записей кандидатов с упущено 0 (до решения 197) в отчёт не входит: {}.",
            t.hidden
        ));
    }
    if t.deals_old.count > 0 {
        notes.push(format!(
            "Сделок, посчитанных после нашей покупки (записи до решения 217: стакан уже съеден нашей покупкой — потенциал занижен): {}. Они — отдельной строкой и на листе «Сделки — занижено», в таблицу потолков не входят.",
            t.deals_old.count
        ));
    }
    if let Some(off) = &data.off {
        notes.push(format!("Расчёт без потолка сейчас не идёт: {off}."));
    }
    for (k, n) in notes.iter().enumerate() {
        italic_note(ws, row + 2 + k as u32, n)?;
    }
    Ok(())
}
