//! Файл: report/cap_summary.rs
//! Что делает: решение 256 — лист «Итоги» отчёта «Сделки, лимиты и капитал»: четыре таблицы ТОЛЬКО формулами
//! (COUNTIFS / SUMIFS / _xlfn.MAXIFS / AVERAGEIFS по листам «Сделки» и «Упущено из-за потолка», без готовых чисел;
//! без XLOOKUP / FILTER / UNIQUE — считаются в Excel и LibreOffice). Строки — направления, которые встречаются за
//! период на любом из двух листов (список приходит готовым, по алфавиту).
//! 1. «Что ограничивает объём»: эпизодов, сколько раз ограничил выгодный объём, лимит на сделку, свободный баланс.
//! 2. «Модель против факта» (сделки с покупкой и итогом): сделок, план, факт, факт / план, минусовых и их сумма.
//! 3. «Сколько добавил бы больший лимит»: прибыль модели на нашем бюджете и на каждом потолке; прибавка между соседними
//!    потолками.
//! 4. «Сколько держать на балансе»: максимум одновременно открытых сделок с покупкой (столбец Q «Сделок»), средняя
//!    длительность, с; рекомендованный баланс = лимит × максимум одновременных (лимиты — строка над таблицей, значения
//!    uncapped.ladder_usdt).
//!
//! Таблицы 3 и 4 — report/cap_capital.rs.
//! Кто вызывает: report/cap_xlsx.rs.
//! От чего зависит: rust_xlsxwriter, report/results_sheet.rs (head, money), cap_sheets.rs (листы, столбцы, rng),
//! cap_capital.rs.
//! Настройки: нет (потолки — uncapped.ladder_usdt, приходят готовым списком).

use rust_decimal::Decimal;
use rust_xlsxwriter::{Format, Formula, Worksheet, XlsxError};

use super::cap_sheets::{DEALS, MISSED, YES, col, d, rng};
use super::results_sheet::{head, money};

/// Что нужно листу: направления, последние строки диапазонов листов, потолки, заголовок и примечания.
pub struct Ctx<'a> {
    pub dirs: &'a [String],
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
    model_vs_fact(&mut w, cx)?;
    super::cap_capital::ladder(&mut w, cx)?;
    super::cap_capital::balance(&mut w, cx)?;
    Ok(())
}

/// Диапазон столбца листа «Сделки». Ошибок нет.
pub(super) fn dr(cx: &Ctx, c: char) -> String {
    rng(DEALS, &c.to_string(), cx.deals_last)
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

/// Таблица 2 «Модель против факта». Ошибки: XlsxError.
fn model_vs_fact(w: &mut W, cx: &Ctx) -> Result<(), XlsxError> {
    let hdr: Vec<String> = [
        "Направление",
        "Сделок с покупкой и итогом",
        "План, USDT",
        "Факт, USDT",
        "Факт / план",
        "Минусовых сделок",
        "Сумма минусовых, USDT",
    ]
    .map(String::from)
    .to_vec();
    w.table("2. Модель против факта (сделки с покупкой и итогом)", &hdr)?;
    let first = w.row + 1;
    let (dir, bought, plan, fact) = (
        dr(cx, d::DIR),
        dr(cx, d::BOUGHT),
        dr(cx, d::PLAN),
        dr(cx, d::FACT),
    );
    for name in cx.dirs {
        let n = w.row + 1;
        let cond = format!("{dir},$A{n},{bought},\"{YES}\"");
        w.ws.write_string_with_format(w.row, 0, name, &w.int)?;
        let (int, m, pct) = (w.int.clone(), w.money.clone(), w.pct.clone());
        w.f(1, format!("=COUNTIFS({cond},{fact},\"<>\")"), &int)?;
        w.f(2, format!("=SUMIFS({plan},{cond},{fact},\"<>\")"), &m)?;
        w.f(3, format!("=SUMIFS({fact},{cond},{fact},\"<>\")"), &m)?;
        w.f(4, format!("=IF(C{n}=0,\"\",D{n}/C{n})"), &pct)?;
        w.f(5, format!("=COUNTIFS({cond},{fact},\"<0\")"), &int)?;
        w.f(6, format!("=SUMIFS({fact},{cond},{fact},\"<0\")"), &m)?;
        w.row += 1;
    }
    let n = w.row + 1;
    total(
        w,
        first,
        &[(1, false), (2, true), (3, true), (5, false), (6, true)],
    )?;
    w.row -= 1;
    let pct = w.pct.clone().set_bold();
    w.f(4, format!("=IF(C{n}=0,\"\",D{n}/C{n})"), &pct)?;
    w.row += 1;
    Ok(())
}
