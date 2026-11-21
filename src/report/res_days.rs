//! Файл: report/res_days.rs
//! Что делает: решение 257 (Тигран 11.11 п. 2, 3: «первая — UTC», итог дня отчёта = итогу дня бота) — лист «По дням» (лист «Учёт дня» — начисления
//! сделок в итог дня бота — report/res_ledger.rs) (день UTC): факт дня и оценка дня — SUMIFS по
//! «Учёту дня» (та же сумма, что у бота: core/daily.rs кладёт эти начисления в книгу дня), рядом — итог дня по записи
//! самого бота (таблица daily_result, core/daily_rows.rs) и разница; день в окне целиком или нет; сделки, закончившиеся
//! в этот день (лист «Сделки»). «Итого» — суммы.
//! Кто вызывает: report/results_xlsx.rs.
//! От чего зависит: rust_xlsxwriter, report/day_rows.rs, report/res_deals.rs (C, rng), report/results_sheet.rs,
//! report/rows_time.rs, core/daily_rows.rs (BotDay).
//! Настройки: нет.

use std::collections::BTreeSet;

use chrono::NaiveDate;
use rust_xlsxwriter::{Formula, Worksheet, XlsxError};

use crate::core::daily_rows::BotDay;

use super::day_rows::DayRow;
use super::res_deals::{C, rng};
use super::results_sheet::{base, header, italic_note, money4, red_green};
use super::rows::{Row, f};
use super::rows_time::Window;

/// Части «Учёта дня» (колонка G).
pub const FACT: &str = "факт";
pub const ESTIMATE: &str = "оценка";
/// Источник «восстановлено» (колонка J).
pub const RESTORED: &str = "восстановлено";

/// Дни листа «По дням»: дни начислений, дни конца сделок и дни записей бота, пересекающиеся с окном. Ошибок нет.
pub fn days(rows: &[Row], led: &[DayRow], bot: &[BotDay], w: &Window) -> Vec<NaiveDate> {
    let mut set: BTreeSet<NaiveDate> = led.iter().map(|r| r.date).collect();
    set.extend(rows.iter().filter_map(|r| r.date.parse::<NaiveDate>().ok()));
    let lo = w.from.map(|t| t.date_naive());
    // Конец окна не включается: окно до 00:00 следующего дня — последний день предыдущий.
    let hi =
        w.to.map_or(w.now, |t| t - chrono::Duration::nanoseconds(1))
            .date_naive();
    set.extend(
        bot.iter()
            .map(|b| b.date)
            .filter(|d| lo.is_none_or(|l| *d >= l) && *d <= hi),
    );
    set.into_iter().collect()
}

/// Лист «По дням». Принимает: лист, дни, записи бота, окно, строк «Учёта дня», строк «Сделок». Ошибки: XlsxError.
pub fn by_day(
    ws: &mut Worksheet,
    keys: &[NaiveDate],
    bot: &[BotDay],
    w: &Window,
    led_n: usize,
    n: u32,
) -> Result<(), XlsxError> {
    ws.set_name("По дням")?;
    let hdr = [
        "День (UTC)",
        "Факт дня, USDT",
        "Оценка дня (не факт), USDT",
        "Итог дня, USDT",
        "Итог дня бота (запись бота), USDT",
        "Разница, USDT",
        "День в окне целиком",
        "Из них восстановлено (сделки до решения 257), USDT",
        "Сделок закончилось",
        "Плюс",
        "Минус",
        "Итог факт сделок дня, USDT",
        "Оценка сделок дня, USDT",
    ];
    header(
        ws,
        &hdr,
        &[
            12., 13., 14., 13., 16., 12., 16., 18., 11., 8., 8., 14., 14.,
        ],
    )?;
    let m = (led_n as u32 + 1).max(2);
    let l = |c: char| format!("'Учёт дня'!${c}$2:${c}${m}");
    let (sum, day, part, src) = (l('H'), l('A'), l('G'), l('J'));
    let (dd, fact, est) = (rng(C.date, n), rng(C.fact, n), rng(C.estimate, n));
    let (p, m4) = (base(), money4());
    for (k, d) in keys.iter().enumerate() {
        let (row, j) = (k as u32 + 1, k as u32 + 2);
        ws.write_string_with_format(row, 0, d.to_string(), &p)?;
        let fs = [
            (1, format!("=SUMIFS({sum},{day},A{j},{part},\"{FACT}\")")),
            (
                2,
                format!("=SUMIFS({sum},{day},A{j},{part},\"{ESTIMATE}\")"),
            ),
            (3, format!("=B{j}+C{j}")),
            (5, format!("=IF(ISNUMBER(E{j}),D{j}-E{j},\"\")")),
            (7, format!("=SUMIFS({sum},{day},A{j},{src},\"{RESTORED}\")")),
            (8, format!("=COUNTIFS({dd},A{j})")),
            (9, format!("=COUNTIFS({dd},A{j},{fact},\">0\")")),
            (10, format!("=COUNTIFS({dd},A{j},{fact},\"<0\")")),
            (11, format!("=SUMIFS({fact},{dd},A{j})")),
            (12, format!("=SUMIFS({est},{dd},A{j})")),
        ];
        for (c, x) in fs {
            let fm = if matches!(c, 8..=10) {
                p.clone()
            } else {
                m4.clone()
            };
            ws.write_formula_with_format(row, c, Formula::new(x), &fm)?;
        }
        match bot.iter().find(|b| b.date == *d) {
            Some(b) => {
                ws.write_number_with_format(row, 4, f(b.final_usd + b.provisional_usd), &m4)?
            }
            None => ws.write_string_with_format(row, 4, "нет записи", &p)?,
        };
        let whole = if w.whole_day(*d) {
            "да"
        } else {
            "нет — часть дня"
        };
        ws.write_string_with_format(row, 6, whole, &p)?;
    }
    let tr = keys.len() as u32 + 1;
    let t = tr + 1;
    ws.write_string_with_format(tr, 0, "Итого", &p.clone().set_bold())?;
    for c in [1u16, 2, 3, 4, 5, 7, 8, 9, 10, 11, 12] {
        let col = (b'A' + c as u8) as char;
        let fm = if matches!(c, 8..=10) {
            p.clone()
        } else {
            m4.clone()
        };
        if keys.is_empty() {
            ws.write_number_with_format(tr, c, 0, &fm.set_bold())?;
        } else {
            let x = format!("=SUM({col}2:{col}{})", t - 1);
            ws.write_formula_with_format(tr, c, Formula::new(x), &fm.set_bold())?;
        }
    }
    red_green(ws, 1, 3, tr, 3)?;
    let notes = [
        "Итог дня — как у бота (core/daily.rs, решения 53, 152, 173, 236): факт — закрытия сделок в день закрытия, поправки затрат возврата USDT и газа после закрытия — в день поправки; оценка — остановленные и «Беру на себя», учтённые в этот день; разбор в другой день — коррекция (факт − оценка) в день разбора. Источник — лист «Учёт дня».",
        "Итог дня бота (запись бота) — строка таблицы daily_result за этот день (то же число, что «Итог дня» в интерфейсе и Telegram для этого дня). «нет записи» — бот в этот день итог не записывал.",
        "Разница ≠ 0: день не целиком в окне выгрузки, или в дне есть «восстановлено» (сделки до решения 257: их поправки после закрытия и прежние оценки по дням не разнесены).",
        "«Сделок закончилось», «Плюс», «Минус», «Итог факт / Оценка сделок дня» — по листу «Сделки» (день конца сделки); их итог включает поправки после закрытия (в любой день), поэтому может отличаться от «Факта дня».",
    ];
    for (k, x) in notes.iter().enumerate() {
        italic_note(ws, tr + 2 + k as u32, x)?;
    }
    Ok(())
}
