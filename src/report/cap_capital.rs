//! Файл: report/cap_capital.rs
//! Что делает: решение 256 (с дополнением — аудит отчёта, п. 2, 4, 9) — таблицы 3–5 листа «Итоги» отчёта «Сделки,
//! лимиты и капитал» (только формулы):
//! 3. «Сколько добавил бы больший лимит»: SUMIFS прибыли модели на нашем бюджете и на каждом потолке по листу
//!    «Упущено из-за потолка»; прибавка между соседними потолками — разность ячеек.
//! 4. «Сколько держать на балансе» — ПО ПЛОЩАДКЕ денег покупки (столбец W листа «Сделки», report/cap_hold.rs), не по
//!    направлению: максимум одновременно занятого, USDT, и максимум одновременно открытых сделок — формулами массива
//!    MAX(IF(…)) по столбцам AA / AB «Сделок» (Excel 2007+ и LibreOffice; _xlfn.MAXIFS нет в Excel до 2019);
//!    занятость — до возврата USDT на площадку покупки, остановленные с монетами — заняты, сделки, начатые до окна, —
//!    в расчёте. «Итого»: сумма максимумов по площадкам (каждая площадка держит свой максимум отдельно — деньги между
//!    площадками мгновенно не переходят) и отдельно максимум одновременно занятого на всех площадках вместе (AC);
//!    максимумы разных площадок не выдаются за одновременную потребность.
//! 5. «Если бы каждая сделка шла на потолок модели»: потолок модели uncapped.ladder_usdt × максимум одновременно
//!    открытых сделок площадки (строка потолков подписана как потолок модели, не как лимит на сделку money.*).
//! Кто вызывает: report/cap_summary.rs (summary).
//! От чего зависит: rust_xlsxwriter, cap_summary.rs (W, Ctx, total, dr, mr), cap_sheets.rs (YES, cap_text, col, d).
//! Настройки: нет (потолки — uncapped.ladder_usdt, приходят готовым списком).

use rust_decimal::prelude::ToPrimitive;
use rust_xlsxwriter::{Formula, XlsxError};

use super::cap_sheets::{YES, cap_text, col, d};
use super::cap_summary::{Ctx, W, dr, mr, total};

/// Таблица 3 «Сколько добавил бы больший лимит». Ошибки: XlsxError.
pub(super) fn ladder(w: &mut W, cx: &Ctx) -> Result<(), XlsxError> {
    let k = cx.ladder.len();
    let mut hdr = vec![
        "Направление".to_string(),
        "Прибыль на нашем бюджете (модель)".to_string(),
    ];
    for c in cx.ladder {
        hdr.push(format!("Прибыль при лимите {} USDT (модель)", cap_text(*c)));
    }
    for p in cx.ladder.windows(2) {
        hdr.push(format!(
            "Прибавка {} → {} USDT (модель)",
            cap_text(p[0]),
            cap_text(p[1])
        ));
    }
    w.table(
        "3. Сколько добавил бы больший лимит (модель, эпизоды)",
        &hdr,
    )?;
    let first = w.row + 1;
    for dir in cx.dirs {
        let n = w.row + 1;
        w.ws.write_string_with_format(w.row, 0, dir, &w.int)?;
        let m = w.money.clone();
        // «Упущено»: E — на нашем бюджете, F… — потолки.
        for i in 0..=k as u16 {
            let f = format!("=SUMIFS({},{},$A{n})", mr(cx, 4 + i), mr(cx, 1));
            w.f(1 + i, f, &m)?;
        }
        for i in 1..k as u16 {
            let (a, b) = (col(1 + i), col(2 + i));
            w.f(2 + k as u16 + i - 1, format!("={b}{n}-{a}{n}"), &m)?;
        }
        w.row += 1;
    }
    // Столбцы: 1 — наш бюджет, 2..=1+k — потолки, дальше k − 1 прибавок: всего до 2k (без потолков — один).
    let cols: Vec<(u16, bool)> = (1..=(2 * k as u16).max(1)).map(|c| (c, true)).collect();
    total(w, first, &cols)
}

/// Формула массива в ячейку строки курсора. Ошибки: XlsxError.
fn arr(w: &mut W, c: u16, f: String, fmt: &rust_xlsxwriter::Format) -> Result<(), XlsxError> {
    w.ws.write_array_formula_with_format(w.row, c, w.row, c, Formula::new(f), fmt)?;
    Ok(())
}

/// Условие массива «площадка строки и учёт в занятости». Ошибок нет.
fn cond(cx: &Ctx, n: u32) -> String {
    format!(
        "({}=$A{n})*({}=\"{YES}\")",
        dr(cx, d::PLACE),
        dr(cx, d::COUNTED)
    )
}

/// Таблица 4 «Сколько держать на балансе» (см. шапку). Ошибки: XlsxError.
pub(super) fn balance(w: &mut W, cx: &Ctx) -> Result<(), XlsxError> {
    let hdr: Vec<String> = [
        "Где лежат деньги покупки",
        "Сделок с покупкой в расчёте",
        "Максимум одновременно занятого, USDT",
        "Максимум одновременно открытых сделок",
        "Средняя длительность сделки, с",
        "Сделок с покупкой вне расчёта (конец / возврат не найдены)",
    ]
    .map(String::from)
    .to_vec();
    w.table(
        "4. Сколько держать на балансе — по площадке денег покупки",
        &hdr,
    )?;
    let first = w.row + 1;
    let (place, counted, bought) = (dr(cx, d::PLACE), dr(cx, d::COUNTED), dr(cx, d::BOUGHT));
    let (busy, open, dur) = (dr(cx, d::BUSY), dr(cx, d::OPEN_N), dr(cx, d::DUR));
    for name in cx.places {
        let n = w.row + 1;
        w.ws.write_string_with_format(w.row, 0, name, &w.int)?;
        let (int, m) = (w.int.clone(), w.money.clone());
        let c = cond(cx, n);
        w.f(
            1,
            format!("=COUNTIFS({place},$A{n},{counted},\"{YES}\")"),
            &int,
        )?;
        arr(w, 2, format!("=IF(B{n}=0,\"\",MAX(IF({c},{busy})))"), &m)?;
        arr(w, 3, format!("=IF(B{n}=0,\"\",MAX(IF({c},{open})))"), &int)?;
        let avg = format!("AVERAGEIFS({dur},{place},$A{n},{counted},\"{YES}\",{dur},\"<>\")");
        w.f(4, format!("=IFERROR(ROUND({avg},0),\"\")"), &int)?;
        w.f(
            5,
            format!("=COUNTIFS({place},$A{n},{bought},\"{YES}\",{counted},\"Нет\")"),
            &int,
        )?;
        w.row += 1;
    }
    total(w, first, &[(1, false), (2, true), (5, false)])?;
    w.row -= 1;
    w.ws.write_string_with_format(
        w.row,
        0,
        "Итого: сумма максимумов по площадкам (каждая держит свой максимум отдельно)",
        &w.bold,
    )?;
    w.row += 1;
    w.ws.write_string_with_format(
        w.row,
        0,
        "Максимум одновременно занятого на всех площадках вместе, USDT",
        &w.bold,
    )?;
    let all = dr(cx, d::BUSY_ALL);
    let m = w.money.clone().set_bold();
    let f = format!("=IF(COUNTIF({counted},\"{YES}\")=0,\"\",MAX(IF({counted}=\"{YES}\",{all})))");
    arr(w, 2, f, &m)?;
    w.row += 1;
    Ok(())
}

/// Таблица 5 «Если бы каждая сделка шла на потолок модели» (см. шапку). Ошибки: XlsxError.
pub(super) fn model_caps(w: &mut W, cx: &Ctx) -> Result<(), XlsxError> {
    if cx.ladder.is_empty() {
        return Ok(());
    }
    w.row += 1;
    let title = "5. Если бы каждая сделка шла на потолок модели «без потолка» (uncapped.ladder_usdt), а не на факт";
    w.ws.write_string_with_format(w.row, 0, title, &w.bold)?;
    w.row += 1;
    // Строка потолков модели (значения uncapped.ladder_usdt) — на неё ссылаются формулы.
    let caps_row = w.row + 1;
    let label = "Потолок модели uncapped.ladder_usdt, USDT →";
    w.ws.write_string_with_format(w.row, 1, label, &w.bold)?;
    for (i, c) in cx.ladder.iter().enumerate() {
        let v = c.to_f64().unwrap_or(0.0);
        w.ws.write_number_with_format(w.row, 2 + i as u16, v, &w.money)?;
    }
    w.row += 1;
    let mut hdr = vec![
        "Где лежат деньги покупки".to_string(),
        "Максимум одновременно открытых сделок".to_string(),
    ];
    for c in cx.ladder {
        hdr.push(format!(
            "Баланс при потолке модели {} USDT = потолок × максимум открытых, USDT",
            cap_text(*c)
        ));
    }
    for (c, h) in hdr.iter().enumerate() {
        w.ws.write_string_with_format(w.row, c as u16, h, &w.head)?;
    }
    w.ws.set_row_height(w.row, 45.)?;
    w.row += 1;
    let first = w.row + 1;
    let open = dr(cx, d::OPEN_N);
    for name in cx.places {
        let n = w.row + 1;
        w.ws.write_string_with_format(w.row, 0, name, &w.int)?;
        let (int, m) = (w.int.clone(), w.money.clone());
        arr(w, 1, format!("=N(MAX(IF({},{open})))", cond(cx, n)), &int)?;
        for i in 0..cx.ladder.len() as u16 {
            let l = col(2 + i);
            w.f(2 + i, format!("={l}${caps_row}*$B{n}"), &m)?;
        }
        w.row += 1;
    }
    let cols: Vec<(u16, bool)> = (0..cx.ladder.len() as u16).map(|i| (2 + i, true)).collect();
    total(w, first, &cols)
}
