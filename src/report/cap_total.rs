//! Файл: report/cap_total.rs
//! Что делает: решение 256 (дополнение — аудит отчёта, п. 3) — таблица 2 «Итог периода» листа «Итоги» отчёта «Сделки,
//! лимиты и капитал» (только формулы по листу «Сделки»): сделки «В итоге периода» — ровно отбор «Итогов сделок»
//! (report/rows.rs selected: закончилась в окне и двигала деньги, в том числе без покупки с затратами, остановленные и
//! «Беру на себя»); по направлению — сделок, закрытых, план закрытых, итог по факту, оценка (не факт), факт + оценка
//! (= «Итог факт + оценка» листа «Итог» «Итогов сделок» за то же окно), факт / план закрытых, затраты сделок, затраты
//! возврата USDT, минусовые (факт).
//! Кто вызывает: report/cap_summary.rs (summary).
//! От чего зависит: rust_xlsxwriter, cap_summary.rs (W, Ctx, total, dr), cap_sheets.rs (YES, d).
//! Настройки: нет.

use rust_xlsxwriter::XlsxError;

use super::cap_sheets::{YES, d};
use super::cap_summary::{Ctx, W, dr, total};

/// Таблица 2 «Итог периода» (сделки «В итоге периода» = отбор «Итогов сделок»). Ошибки: XlsxError.
pub(super) fn period_total(w: &mut W, cx: &Ctx) -> Result<(), XlsxError> {
    let hdr: Vec<String> = [
        "Направление",
        "Сделок в итоге",
        "из них закрыты (факт)",
        "План закрытых, USDT",
        "Итог по факту, USDT",
        "Оценка (не факт), USDT",
        "Итог факт + оценка, USDT",
        "Факт / план (закрытые)",
        "Затраты сделок, USDT",
        "Затраты возврата USDT, USDT",
        "Минусовых (факт)",
        "Сумма минусовых, USDT",
    ]
    .map(String::from)
    .to_vec();
    w.table(
        "2. Итог периода — те же сделки и тот же итог, что «Итоги сделок» (факт и оценка отдельно)",
        &hdr,
    )?;
    let first = w.row + 1;
    let r = |c: &str| dr(cx, c);
    let (fact, plan) = (r(d::FACT), r(d::PLAN));
    for name in cx.dirs {
        let n = w.row + 1;
        let cond = format!("{},$A{n},{},\"{YES}\"", r(d::DIR), r(d::IN_TOTAL));
        w.ws.write_string_with_format(w.row, 0, name, &w.int)?;
        let (int, m, pct) = (w.int.clone(), w.money.clone(), w.pct.clone());
        let fs = [
            (1, format!("=COUNTIFS({cond})"), &int),
            (2, format!("=COUNTIFS({cond},{fact},\"<>\")"), &int),
            (3, format!("=SUMIFS({plan},{cond},{fact},\"<>\")"), &m),
            (4, format!("=SUMIFS({fact},{cond})"), &m),
            (5, format!("=SUMIFS({},{cond})", r(d::ESTIMATE)), &m),
            (6, format!("=E{n}+F{n}"), &m),
            (7, format!("=IF(D{n}=0,\"\",E{n}/D{n})"), &pct),
            (8, format!("=SUMIFS({},{cond})", r(d::SPENT)), &m),
            (9, format!("=SUMIFS({},{cond})", r(d::RET_COST)), &m),
            (10, format!("=COUNTIFS({cond},{fact},\"<0\")"), &int),
            (11, format!("=SUMIFS({fact},{cond},{fact},\"<0\")"), &m),
        ];
        for (c, f, fmt) in fs {
            w.f(c, f, fmt)?;
        }
        w.row += 1;
    }
    let n = w.row + 1;
    let sums: Vec<(u16, bool)> = [(1, false), (2, false), (10, false)]
        .into_iter()
        .chain([3, 4, 5, 6, 8, 9, 11].map(|c| (c, true)))
        .collect();
    total(w, first, &sums)?;
    w.row -= 1;
    let pct = w.pct.clone().set_bold();
    w.f(7, format!("=IF(D{n}=0,\"\",E{n}/D{n})"), &pct)?;
    w.row += 1;
    Ok(())
}
