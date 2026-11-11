//! Файл: report/cap_deals.rs
//! Что делает: решение 256 (с дополнением — аудит отчёта) — лист «Сделки» отчёта «Сделки, лимиты и капитал»: строка на
//! сделку (report/cap_rows.rs), значения учёта, адреса (cap_addr.rs) и формулы:
//! - «Длительность, с» = ROUND((Конец − Начало) × 86400, 0);
//! - занятость денег покупки в момент начала сделки (только строки «Учёт в занятости» = «Да», cap_hold.rs): сделки, у
//!   которых занятость учитывается, начало ≤ начала этой и деньги ещё не свободны к её началу (свободны ≥ начала или
//!   пусто — заняты до выгрузки), включая её саму:
//!   «Занято на этой площадке» = SUMIFS(Вложено; та же площадка, …) + SUMIFS(…; свободны "");
//!   «Открыто сделок на этой площадке» — то же COUNTIFS; «Занято на всех площадках» — SUMIFS без условия площадки.
//!   Максимум занятого во времени достигается в момент начала какой-то сделки — поэтому максимум этих столбцов по
//!   площадке = максимум одновременно занятого (лист «Итоги», report/cap_capital.rs).
//!
//! Время — число Excel с 10 знаками (cap_sheets.rs serial): условие "<="&I находит саму сделку.
//! Кто вызывает: report/cap_xlsx.rs.
//! От чего зависит: rust_xlsxwriter, report/results_sheet.rs (header, money), cap_sheets.rs, cap_rows.rs.
//! Настройки: нет.

use rust_xlsxwriter::{Formula, Worksheet, XlsxError};

use super::cap_rows::DealRow;
use super::cap_sheets::{DEALS, YES, d, last_row, plain, rng, serial, time_fmt};
use super::results_sheet::{header, money};

/// Шапка листа (буквы столбцов для формул — cap_sheets.rs d).
pub const HDR: [&str; 38] = [
    "№ сделки",
    "Монета",
    "Контракт монеты (адрес BSC)",
    "Биржа покупки",
    "Биржа продажи",
    "Направление",
    "Покупка состоялась",
    "Статус",
    "Начало (UTC)",
    "Конец (UTC)",
    "Длительность, с",
    "Конец — пометка",
    "Вложено, USDT",
    "Получено (выручка), USDT",
    "Затраты сделки, USDT",
    "Затраты возврата USDT, USDT",
    "Прибыль по плану, USDT",
    "Итог по факту, USDT",
    "Оценка (не факт), USDT",
    "В итоге периода (как «Итоги сделок»)",
    "Почему не в итоге",
    "Возврат USDT (состояние)",
    "Где лежат деньги покупки",
    "Деньги свободны на площадке покупки (UTC)",
    "Учёт в занятости",
    "Занятость — пояснение",
    "Занято на этой площадке в момент начала, USDT",
    "Открыто сделок на этой площадке в момент начала",
    "Занято на всех площадках в момент начала, USDT",
    "Кошелёк DEX сделки (адрес)",
    "Пулы маршрута покупки (DEX)",
    "Пулы маршрута продажи (DEX)",
    "Покупка: id ордера биржи",
    "Покупка: хеш транзакции DEX",
    "Продажи: id ордеров биржи",
    "Продажи: хеши транзакций DEX",
    "Перевод монеты: вывод (площадка, id, хеш)",
    "Перевод монеты: зачисление (площадка, id, хеш)",
];

/// Ширины столбцов.
const WIDTHS: [f64; 38] = [
    26., 12., 44., 13., 13., 16., 11., 22., 18., 18., 12., 22., 13., 13., 13., 13., 14., 14., 14.,
    12., 30., 36., 26., 18., 10., 40., 16., 14., 16., 44., 60., 60., 22., 44., 22., 44., 60., 60.,
];

/// Да / Нет. Ошибок нет.
fn yes(b: bool) -> &'static str {
    if b { YES } else { "Нет" }
}

/// Лист «Сделки». Принимает: лист, строки. Ошибки: XlsxError.
pub fn deals(ws: &mut Worksheet, rows: &[DealRow]) -> Result<(), XlsxError> {
    ws.set_name(DEALS)?;
    header(ws, &HDR, &WIDTHS)?;
    let (p, m, t) = (plain(), money(), time_fmt());
    let last = last_row(rows.len());
    let r = |c: &str| rng(DEALS, c, last);
    for (i, x) in rows.iter().enumerate() {
        let (row, n) = (i as u32 + 1, i as u32 + 2);
        let a = &x.addr;
        let texts: [(u16, &str); 23] = [
            (0, &x.id),
            (1, &x.coin),
            (2, &x.contract),
            (3, &x.buy),
            (4, &x.sell),
            (5, &x.dir),
            (6, yes(x.bought)),
            (7, &x.status),
            (11, &x.end_note),
            (19, yes(x.in_total)),
            (20, &x.why_not),
            (21, &x.ret),
            (22, &x.hold.place),
            (24, yes(x.hold.counted)),
            (25, &x.hold.note),
            (29, &a.wallet),
            (30, &a.buy_pools),
            (31, &a.sell_pools),
            (32, &a.buy_order),
            (33, &a.buy_tx),
            (34, &a.sell_orders),
            (35, &a.sell_txs),
            (36, &a.withdraws),
        ];
        for (c, s) in texts {
            ws.write_string_with_format(row, c, s, &p)?;
        }
        ws.write_string_with_format(row, 37, &a.deposits, &p)?;
        ws.write_number_with_format(row, 8, serial(x.start), &t)?;
        if let Some(e) = x.end {
            ws.write_number_with_format(row, 9, serial(e), &t)?;
            let f = format!("=ROUND(({e}{n}-{s}{n})*86400,0)", e = d::END, s = d::START);
            ws.write_formula_with_format(row, 10, Formula::new(f), &p)?;
        }
        let nums = [
            (12, Some(x.invested)),
            (13, Some(x.proceeds)),
            (14, Some(x.spent)),
            (15, Some(x.ret_cost)),
            (16, Some(x.plan)),
            (17, x.fact),
            (18, x.estimate),
        ];
        for (c, v) in nums {
            if let Some(v) = v {
                ws.write_number_with_format(row, c, v, &m)?;
            }
        }
        if let Some(u) = x.hold.until {
            ws.write_number_with_format(row, 23, serial(u), &t)?;
        }
        if x.hold.counted {
            // Занятые к началу этой сделки: учитываются, начаты не позже, свободны не раньше её начала или ещё заняты.
            let st = d::START;
            let base = format!("{},\"{YES}\",{},\"<=\"&{st}{n}", r(d::COUNTED), r(d::START));
            let place = format!("{},{}{n},", r(d::PLACE), d::PLACE);
            let free = r(d::FREE_AT);
            let busy = |func: &str, sum: &str, by_place: bool| {
                let pl = if by_place { place.as_str() } else { "" };
                format!(
                    "={func}({sum}{pl}{base},{free},\">=\"&{st}{n})+{func}({sum}{pl}{base},{free},\"\")"
                )
            };
            let inv = format!("{},", r(d::INVESTED));
            ws.write_formula_with_format(row, 26, Formula::new(busy("SUMIFS", &inv, true)), &m)?;
            ws.write_formula_with_format(row, 27, Formula::new(busy("COUNTIFS", "", true)), &p)?;
            ws.write_formula_with_format(row, 28, Formula::new(busy("SUMIFS", &inv, false)), &m)?;
        }
    }
    ws.autofilter(0, 0, rows.len() as u32, (HDR.len() - 1) as u16)?;
    Ok(())
}
