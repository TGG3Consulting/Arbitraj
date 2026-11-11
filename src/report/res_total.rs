//! Файл: report/res_total.rs
//! Что делает: решение 257 — лист «Итог» отчёта «Итоги сделок» (вынесено из results_xlsx.rs — размер файла): окно
//! выгрузки (точно, UTC, из параметров выгрузки) и время выгрузки; число сделок, закрытых (факт) и с оценкой; плюс /
//! минус, винрейт, итог факт и оценка отдельно, план и его части (запас на движение цены, возврат USDT), «факт − план»
//! одинакового состава, оборот, длительность; итог дней как у бота и по записям бота (лист «По дням»), балансы (лист
//! «Слепок по балансам»); что не прочитано; пояснения по коду (res_notes.rs). Всё — формулами по листам книги.
//! Кто вызывает: report/results_xlsx.rs.
//! От чего зависит: rust_xlsxwriter, report/res_deals.rs (C, rng), report/results_sheet.rs, report/res_notes.rs.
//! Настройки: нет.

use rust_xlsxwriter::{Format, Formula, Worksheet, XlsxError};

use super::res_deals::{C, rng};
use super::results_sheet::{base, header, italic_note};
use super::results_xlsx::Input;

/// Лист «Итог». Принимает: лист, вход, последнюю строку «Сделок», строку «Итого» листа «По дням» (1-based), строку общего
/// итога «Слепка», время выгрузки. Ошибки: XlsxError.
pub fn total(
    ws: &mut Worksheet,
    x: &Input,
    n: u32,
    day_t: u32,
    snap_t: u32,
    at: &str,
) -> Result<(), XlsxError> {
    ws.set_name("Итог")?;
    header(ws, &["Показатель", "Значение"], &[52., 40.])?;
    let r = |c: &str| rng(c, n);
    let (p, fa) = (r(C.fact), r(C.fact));
    let items: Vec<(&str, String, &str)> = vec![
        ("Окно выгрузки (UTC)", x.window.text(), "t"),
        ("Выгрузка (UTC)", format!("{at} UTC"), "t"),
        (
            "Сделок закончилось в окне",
            format!("=COUNTA({})", r("A")),
            "n",
        ),
        ("из них закрыты (итог — факт)", format!("=COUNT({p})"), "n"),
        (
            "остановлены / «Беру на себя» (итог — оценка)",
            format!("=COUNT({})", r(C.estimate)),
            "n",
        ),
        ("Плюсовых", format!("=COUNTIF({p},\">0\")"), "n"),
        ("Минусовых", format!("=COUNTIF({p},\"<0\")"), "n"),
        (
            "Винрейт (плюсовых / закрытых)",
            "=IFERROR(B7/B5,0)".into(),
            "%",
        ),
        ("Итог факт, USDT", format!("=SUM({p})"), "m"),
        (
            "Оценка позиций (не факт), USDT",
            format!("=SUM({})", r(C.estimate)),
            "m",
        ),
        ("Итог факт + оценка, USDT", "=B10+B11".into(), "m"),
        (
            "План закрытых, USDT",
            format!("=SUMIFS({},{fa},\"<>\")", r(C.plan)),
            "m",
        ),
        (
            "Запас на движение цены в плане закрытых, USDT",
            format!("=SUMIFS({},{fa},\"<>\")", r(C.reserve)),
            "m",
        ),
        (
            "Возврат USDT в плане закрытых, USDT",
            format!("=SUMIFS({},{fa},\"<>\")", r(C.plan_ret)),
            "m",
        ),
        (
            "Возврат USDT в факте закрытых, USDT",
            format!("=SUMIFS({},{fa},\"<>\")", r(C.ret_fact)),
            "m",
        ),
        (
            "Факт − план (до запаса, без возврата USDT), USDT",
            format!("=SUM({})", r(C.cmp)),
            "m",
        ),
        (
            "Средний итог закрытой сделки, USDT",
            "=IFERROR(B10/B5,0)".into(),
            "m",
        ),
        ("Сумма плюсов, USDT", format!("=SUMIF({p},\">0\")"), "m"),
        ("Сумма минусов, USDT", format!("=SUMIF({p},\"<0\")"), "m"),
        (
            "Лучшая сделка, USDT",
            format!("=IF(B5=0,\"\",MAX({p}))"),
            "m",
        ),
        (
            "Худшая сделка, USDT",
            format!("=IF(B5=0,\"\",MIN({p}))"),
            "m",
        ),
        (
            "Вложено в закрытые (оборот), USDT",
            format!("=SUMIFS({},{fa},\"<>\")", r(C.invested)),
            "m",
        ),
        ("Доходность на оборот", "=IFERROR(B10/B23,0)".into(), "%%"),
        (
            "Медиана длительности (старт → конец), с",
            format!("=IF(COUNT({0})=0,\"\",MEDIAN({0}))", r(C.duration)),
            "n",
        ),
        match x.long_deal_s {
            Ok(s) => (
                "Сделок дольше порога report.long_deal_s",
                format!("=COUNTIF({},\">{s}\")", r(C.duration)),
                "n",
            ),
            Err(e) => ("Сделок дольше порога report.long_deal_s", e.clone(), "t"),
        },
        (
            "Итог дней за окно, как у бота (лист «По дням»), USDT",
            format!("='По дням'!D{day_t}"),
            "m",
        ),
        (
            "Итог дней по записям бота (лист «По дням»), USDT",
            format!("='По дням'!E{day_t}"),
            "m",
        ),
        (
            "Балансы на момент выгрузки, USDT (лист «Слепок по балансам»)",
            format!("='Слепок по балансам'!F{snap_t}"),
            "m",
        ),
        (
            "Балансы: полнота слепка",
            format!("='Слепок по балансам'!G{snap_t}"),
            "n",
        ),
    ];
    let plain = base();
    for (k, (name, f, kind)) in items.iter().enumerate() {
        let row = k as u32 + 1;
        let fmt = match *kind {
            "%" => base().set_bold().set_num_format("0.0%"),
            "%%" => base().set_bold().set_num_format("0.00%"),
            "m" => base()
                .set_bold()
                .set_num_format("#,##0.0000;\\-#,##0.0000;0.0000"),
            _ => base().set_bold(),
        };
        ws.write_string_with_format(row, 0, *name, &plain)?;
        if *kind == "t" {
            ws.write_string_with_format(row, 1, f, &fmt)?;
        } else {
            ws.write_formula_with_format(row, 1, Formula::new(f.as_str()), &fmt)?;
        }
    }
    let mut row = items.len() as u32 + 2;
    for t in x.problems {
        italic_note(ws, row, &format!("Не прочитано: {t}"))?;
        row += 1;
    }
    let bold = Format::new().set_font_name("Arial").set_bold();
    ws.write_string_with_format(row, 0, "Пояснения (по коду бота)", &bold)?;
    for (k, (h, t)) in super::res_notes::NOTES.iter().enumerate() {
        italic_note(ws, row + 1 + k as u32, &format!("{h}. {t}"))?;
    }
    Ok(())
}
