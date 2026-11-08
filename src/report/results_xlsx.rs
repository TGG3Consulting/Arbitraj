//! Файл: report/results_xlsx.rs
//! Что делает: решение 193 (Тигран, 06.11.2026: «кнопка, по нажатию на которую скачается РОВНО ТАКОЙ отчёт … актуальный
//! на момент скачивания») — книга Excel «Итоги сделок» 1 в 1 как arb_results_2026-11-05.xlsx (scratchpad xl/build.py):
//! листы «Итог», «Сделки», «По дням», «По биржам», «По направлениям», «По монетам», «По часам UTC»; те же заголовки,
//! формулы (COUNTIFS / SUMIFS / _xlfn.MAXIFS / _xlfn.MINIFS по листу «Сделки»), шрифт Arial, шапка белым по 1F4E78,
//! ширины столбцов, форматы чисел, закрепление первой строки, фильтр, красный / зелёный итог, примечания курсивом.
//! Значения формул Excel считает при открытии (как у книги openpyxl). Отличается только то, что меняется со временем:
//! строки сделок и время выгрузки в примечании. Списки биржи: BingX, MEXC, KuCoin, Gate, DEX — как в отчёте; биржа,
//! которой там не было, но по ней есть сделки, — перед DEX.
//! Кто вызывает: report/mod.rs (build).
//! От чего зависит: rust_xlsxwriter, report/rows.rs, report/results_sheet.rs.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_xlsxwriter::{Format, Formula, Workbook, Worksheet, XlsxError};

use super::results_sheet::{
    Key, header, italic_note, money, money4, red_green, summary, summary_venues,
};
use super::rows::Row;

/// Диапазон столбца листа «Сделки» для формул: Сделки!$C$2:$C$N (без кавычек — как в отчёте).
pub fn rng(c: char, n: u32) -> String {
    format!("Сделки!${c}$2:${c}${n}")
}

/// Книга отчёта. Принимает: строки сделок, время выгрузки. Возвращает: байты .xlsx. Ошибки: XlsxError (запись книги).
pub fn workbook(rows: &[Row], now: DateTime<Utc>) -> Result<Vec<u8>, XlsxError> {
    let n = rows.len() as u32 + 1;
    let mut wb = Workbook::new();
    total(wb.add_worksheet(), n, now)?;
    deals(wb.add_worksheet(), rows, n)?;
    let days = uniq(rows.iter().map(|r| r.date.clone()));
    summary(
        wb.add_worksheet(),
        "По дням",
        "День (UTC)",
        &keys(&days),
        'A',
        n,
        None,
    )?;
    summary_venues(wb.add_worksheet(), &venues(rows), n)?;
    summary(
        wb.add_worksheet(),
        "По направлениям",
        "Направление",
        &keys(&dirs(rows)),
        'G',
        n,
        None,
    )?;
    summary(
        wb.add_worksheet(),
        "По монетам",
        "Монета",
        &keys(&coins(rows)),
        'D',
        n,
        Some("Отсортировано по итогу факт (на момент выгрузки)."),
    )?;
    let hours: Vec<Key> = (0..24).map(Key::Num).collect();
    summary(
        wb.add_worksheet(),
        "По часам UTC",
        "Час (UTC)",
        &hours,
        'C',
        n,
        None,
    )?;
    wb.save_to_buffer()
}

fn keys(v: &[String]) -> Vec<Key> {
    v.iter().cloned().map(Key::Text).collect()
}

/// Уникальные значения по порядку возрастания. Ошибок нет.
fn uniq(it: impl Iterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = it.collect();
    v.sort();
    v.dedup();
    v
}

/// Значения в порядке первого появления (как словарь в xl/build.py). Ошибок нет.
fn first_seen(it: impl Iterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for x in it {
        if !v.contains(&x) {
            v.push(x);
        }
    }
    v
}

/// Направления «Купили → Продали» по числу сделок, больше — выше (равные — по первому появлению). Ошибок нет.
fn dirs(rows: &[Row]) -> Vec<String> {
    let mut v = first_seen(rows.iter().map(|r| format!("{} → {}", r.buy, r.sell)));
    let count = |d: &str| {
        rows.iter()
            .filter(|r| format!("{} → {}", r.buy, r.sell) == d)
            .count()
    };
    v.sort_by_key(|d| std::cmp::Reverse(count(d)));
    v
}

/// Монеты по итогу факт, больше — выше (равные — по первому появлению, как в xl/build.py). Ошибок нет.
fn coins(rows: &[Row]) -> Vec<String> {
    let mut v = first_seen(rows.iter().map(|r| r.coin.clone()));
    let sum = |c: &str| {
        rows.iter()
            .filter(|r| r.coin == c)
            .map(|r| r.fact)
            .sum::<f64>()
    };
    v.sort_by(|a, b| sum(b).total_cmp(&sum(a)));
    v
}

/// Биржи: BingX, MEXC, KuCoin, Gate, DEX (как в отчёте); другие биржи со сделками — перед DEX. Ошибок нет.
fn venues(rows: &[Row]) -> Vec<String> {
    let mut v: Vec<String> = ["BingX", "MEXC", "KuCoin", "Gate"]
        .map(String::from)
        .to_vec();
    for name in uniq(rows.iter().flat_map(|r| [r.buy.clone(), r.sell.clone()])) {
        if !v.contains(&name) && name != "DEX" {
            v.push(name);
        }
    }
    v.push("DEX".into());
    v
}

/// Лист «Сделки». Ошибки: XlsxError.
fn deals(ws: &mut Worksheet, rows: &[Row], n: u32) -> Result<(), XlsxError> {
    ws.set_name("Сделки")?;
    let hdr = [
        "Дата (UTC)",
        "Время",
        "Час",
        "Монета",
        "Купили на",
        "Продали на",
        "Направление",
        "Вложено, USDT",
        "План, USDT",
        "Факт, USDT",
        "Плюс/минус",
        "Длительность, с",
        "Факт − план, USDT",
    ];
    header(
        ws,
        &hdr,
        &[
            12., 8., 6., 12., 10., 10., 18., 13., 11., 11., 11., 14., 14.,
        ],
    )?;
    let (plain, money, money4) = (Format::new().set_font_name("Arial"), money(), money4());
    for (k, r) in rows.iter().enumerate() {
        let (row, i) = (k as u32 + 1, k as u32 + 2);
        ws.write_string_with_format(row, 0, &r.date, &plain)?;
        ws.write_string_with_format(row, 1, &r.time, &plain)?;
        ws.write_number_with_format(row, 2, r.hour, &plain)?;
        ws.write_string_with_format(row, 3, &r.coin, &plain)?;
        ws.write_string_with_format(row, 4, &r.buy, &plain)?;
        ws.write_string_with_format(row, 5, &r.sell, &plain)?;
        ws.write_formula_with_format(row, 6, Formula::new(format!("=E{i}&\" → \"&F{i}")), &plain)?;
        ws.write_number_with_format(row, 7, r.invested, &money)?;
        ws.write_number_with_format(row, 8, r.plan, &money4)?;
        ws.write_number_with_format(row, 9, r.fact, &money4)?;
        let pm = format!("=IF(J{i}>0,\"плюс\",IF(J{i}<0,\"минус\",\"ноль\"))");
        ws.write_formula_with_format(row, 10, Formula::new(pm), &plain)?;
        ws.write_number_with_format(row, 11, r.duration_s as f64, &plain)?;
        ws.write_formula_with_format(row, 12, Formula::new(format!("=J{i}-I{i}")), &money4)?;
    }
    ws.autofilter(0, 0, n - 1, 12)?;
    // Решение 215: период без сделок (фильтр from/to) — книга с пустым листом; диапазона цвета нет.
    if !rows.is_empty() {
        red_green(ws, 1, 9, n - 1, 9)?;
    }
    Ok(())
}

/// Лист «Итог». Принимает: лист, последнюю строку «Сделок», время выгрузки. Ошибки: XlsxError.
fn total(ws: &mut Worksheet, n: u32, now: DateTime<Utc>) -> Result<(), XlsxError> {
    ws.set_name("Итог")?;
    header(ws, &["Показатель", "Значение"], &[34., 27.])?;
    let r = |c| rng(c, n);
    let items: Vec<(&str, String)> = vec![
        ("Период", format!("=Сделки!A2&\" — \"&Сделки!A{n}")),
        ("Сделок", format!("=COUNTA({})", r('D'))),
        ("Плюсовых", format!("=COUNTIF({},\">0\")", r('J'))),
        ("Минусовых", format!("=COUNTIF({},\"<0\")", r('J'))),
        ("Винрейт", "=IFERROR(B4/B3,0)".into()),
        ("Итог факт, USDT", format!("=SUM({})", r('J'))),
        ("План, USDT", format!("=SUM({})", r('I'))),
        ("Факт − план, USDT", "=B7-B8".into()),
        ("Средний итог сделки, USDT", "=IFERROR(B7/B3,0)".into()),
        ("Сумма плюсов, USDT", format!("=SUMIF({},\">0\")", r('J'))),
        ("Сумма минусов, USDT", format!("=SUMIF({},\"<0\")", r('J'))),
        ("Лучшая сделка, USDT", format!("=MAX({})", r('J'))),
        ("Худшая сделка, USDT", format!("=MIN({})", r('J'))),
        ("Вложено всего (оборот), USDT", format!("=SUM({})", r('H'))),
        ("Доходность на оборот", "=IFERROR(B7/B15,0)".into()),
        ("Медиана длительности, с", format!("=MEDIAN({})", r('L'))),
        (
            "Сделок дольше 5 минут",
            format!("=COUNTIF({},\">300\")", r('L')),
        ),
    ];
    let plain = Format::new().set_font_name("Arial");
    for (k, (name, f)) in items.iter().enumerate() {
        let row = k as u32 + 1;
        let excel_row = row + 1;
        let fmt = match excel_row {
            6 => Format::new()
                .set_font_name("Arial")
                .set_bold()
                .set_num_format("0.0%"),
            16 => Format::new()
                .set_font_name("Arial")
                .set_bold()
                .set_num_format("0.00%"),
            7..=14 => Format::new()
                .set_font_name("Arial")
                .set_bold()
                .set_num_format("#,##0.00;\\-#,##0.00;0.00"),
            15 => Format::new()
                .set_font_name("Arial")
                .set_bold()
                .set_num_format("#,##0"),
            _ => Format::new().set_font_name("Arial").set_bold(),
        };
        ws.write_string_with_format(row, 0, *name, &plain)?;
        ws.write_formula_with_format(row, 1, Formula::new(f.as_str()), &fmt)?;
    }
    let notes = [
        format!(
            "Источник: учёт сделок бота (GET /api/deals), выгрузка {} UTC; только сделки с вложением > 0.",
            now.format("%d.%m.%Y ~%H:%M")
        ),
        "Факт — итог сделки после всех затрат, включая затраты возврата выручки USDT на площадку покупки (решение 236; возврат «не вернулись» / «перегнано вручную» — без них, стоимость неизвестна); закрытые вручную («Продано вручную») — выручка бота плюс введённая выручка человека.".into(),
        "Длительность — от старта сделки до последнего обновления записи (у разобранных вручную — до разбора, поэтому длинная).".into(),
        "Плюс/минус — по факту; «ноль» не входит ни в плюс, ни в минус.".into(),
    ];
    let first = items.len() as u32 + 2;
    for (k, t) in notes.iter().enumerate() {
        italic_note(ws, first + k as u32, t)?;
    }
    Ok(())
}
