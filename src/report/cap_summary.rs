//! Файл: report/cap_summary.rs
//! Что делает: решение 256 (с дополнением — аудит отчёта) — лист «Итоги» отчёта «Сделки, лимиты и капитал»: заголовок,
//! примечания (report/cap_notes.rs) и таблицы ТОЛЬКО формулами по листам книги (COUNTIFS / SUMIFS / AVERAGEIFS / IF /
//! IFERROR / ROUND, формулы массива MAX(IF(…)); без _xlfn-функций — MAXIFS / MINIFS нет в Excel до 2019, решение 257;
//! без XLOOKUP / FILTER / UNIQUE). Строки таблиц 1–3 — направления периода с обоих листов (по алфавиту).
//! 1. «Что ограничивает объём»: эпизодов, сколько раз ограничил выгодный объём, лимит на сделку, свободный баланс.
//! 2. «Итог периода» (= «Итог факт + оценка» «Итогов сделок» за то же окно).
//!
//! Таблица 2 — report/cap_total.rs; таблицы 3 («Сколько добавил бы больший лимит»), 4 («Сколько держать на балансе»
//! по площадкам) и 5 (потолки модели) — report/cap_capital.rs.
//! Кто вызывает: report/cap_xlsx.rs.
//! От чего зависит: rust_xlsxwriter, report/results_sheet.rs (head, money), cap_sheets.rs (листы, столбцы, rng),
//! cap_capital.rs.
//! Настройки: нет (потолки — uncapped.ladder_usdt, приходят готовым списком).

use rust_decimal::Decimal;
use rust_xlsxwriter::{Format, Formula, Worksheet, XlsxError};

use super::cap_sheets::{DEALS, MISSED, col, rng};
use super::results_sheet::{head, money};

/// Что нужно листу: направления, последние строки диапазонов листов, потолки, заголовок и примечания.
pub struct Ctx<'a> {
    pub dirs: &'a [String],
    /// Площадки денег покупки (столбец «Где лежат деньги покупки» листа «Сделки»), по алфавиту.
    pub places: &'a [String],
    pub deals_last: u32,
    pub missed_last: u32,
    pub ladder: &'a [Decimal],
    pub title: String,
    pub notes: Vec<String>,
}

/// Курсор строк листа и форматы.
pub(super) struct W<'a> {
    pub(super) ws: &'a mut Worksheet,
    pub(super) row: u32,
    pub(super) bold: Format,
    pub(super) head: Format,
    pub(super) money: Format,
    pub(super) int: Format,
    pub(super) pct: Format,
}

impl W<'_> {
    /// Заголовок таблицы жирным и строка шапки. Ошибки: XlsxError.
    pub(super) fn table(&mut self, title: &str, hdr: &[String]) -> Result<(), XlsxError> {
        self.row += 1;
        self.ws
            .write_string_with_format(self.row, 0, title, &self.bold)?;
        self.row += 1;
        for (c, h) in hdr.iter().enumerate() {
            self.ws
                .write_string_with_format(self.row, c as u16, h, &self.head)?;
        }
        self.ws.set_row_height(self.row, 45.)?;
        self.row += 1;
        Ok(())
    }

    /// Формула в ячейку. Ошибки: XlsxError.
    pub(super) fn f(&mut self, c: u16, text: String, fmt: &Format) -> Result<(), XlsxError> {
        self.ws
            .write_formula_with_format(self.row, c, Formula::new(text), fmt)?;
        Ok(())
    }
}

/// Строка «Итого» с SUM по столбцам `sum` от строки `first` (1-based в формуле). Ошибки: XlsxError.
pub(super) fn total(w: &mut W, first: u32, cols: &[(u16, bool)]) -> Result<(), XlsxError> {
    w.ws.write_string_with_format(w.row, 0, "Итого", &w.bold)?;
    for &(c, is_money) in cols {
        let l = col(c);
        let f = format!("=SUM({l}{first}:{l}{})", w.row);
        let fmt = if is_money {
            w.money.clone()
        } else {
            w.int.clone()
        };
        w.f(c, f, &fmt.set_bold())?;
    }
    w.row += 1;
    Ok(())
}

/// Лист «Итоги». Принимает: лист, контекст. Ошибки: XlsxError.
pub fn summary(ws: &mut Worksheet, cx: &Ctx) -> Result<(), XlsxError> {
    ws.set_name("Итоги")?;
    let base = Format::new().set_font_name("Arial");
    let mut w = W {
        ws,
        row: 0,
        bold: base.clone().set_bold(),
        head: head(),
        money: money(),
        int: base.clone(),
        pct: base.clone().set_num_format("0.0%"),
    };
    w.ws.set_column_width(0, 18.)?;
    for c in 1..(4 + 2 * cx.ladder.len() as u16).max(8) {
        w.ws.set_column_width(c, 15.)?;
    }
    w.ws.write_string_with_format(0, 0, &cx.title, &w.bold)?;
    let note = base.clone().set_italic();
    for t in &cx.notes {
        w.row += 1;
        w.ws.write_string_with_format(w.row, 0, t, &note)?;
    }
    w.row += 1;
    limits(&mut w, cx)?;
    super::cap_total::period_total(&mut w, cx)?;
    super::cap_capital::ladder(&mut w, cx)?;
    super::cap_capital::balance(&mut w, cx)?;
    super::cap_capital::model_caps(&mut w, cx)?;
    Ok(())
}

/// Диапазон столбца листа «Сделки». Ошибок нет.
pub(super) fn dr(cx: &Ctx, c: &str) -> String {
    rng(DEALS, c, cx.deals_last)
}

/// Диапазон столбца листа «Упущено» по номеру столбца. Ошибок нет.
pub(super) fn mr(cx: &Ctx, c: u16) -> String {
    rng(MISSED, &col(c), cx.missed_last)
}

/// Таблица 1 «Что ограничивает объём». Ошибки: XlsxError.
fn limits(w: &mut W, cx: &Ctx) -> Result<(), XlsxError> {
    let k = cx.ladder.len() as u16;
    // Столбцы «Упущено»: C — эпизодов; ограничения — после потолков: выгодный объём, лимит на сделку, баланс.
    let src = [2, 5 + k, 6 + k, 7 + k];
    let hdr: Vec<String> = [
        "Направление",
        "Эпизодов",
        "Ограничил выгодный объём",
        "Ограничил лимит на сделку",
        "Ограничил свободный баланс",
    ]
    .map(String::from)
    .to_vec();
    w.table("1. Что ограничивает объём (эпизоды «без потолка»)", &hdr)?;
    let first = w.row + 1;
    for dir in cx.dirs {
        w.ws.write_string_with_format(w.row, 0, dir, &w.int)?;
        for (i, s) in src.iter().enumerate() {
            let f = format!("=SUMIFS({},{},$A{})", mr(cx, *s), mr(cx, 1), w.row + 1);
            let int = w.int.clone();
            w.f(i as u16 + 1, f, &int)?;
        }
        w.row += 1;
    }
    total(w, first, &[(1, false), (2, false), (3, false), (4, false)])
}
