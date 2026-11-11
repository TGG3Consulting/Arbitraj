//! Файл: report/sig_xlsx.rs
//! Что делает: решение 255 — книга Excel «Сигналы и сделки» (часть файла — набор сигналов, у одного очень большого
//! сигнала — доля его объёмных строк, report/sig_parts.rs). Листы: «Сигналы» — колонки ровно как в задаче Тиграна 11.11
//! (MAIN), одна строка на направление окна; «Тики», «Стаканы», «Ликвидность пула» — объёмные строки с ключом сигнала
//! (id окна) и временем; «Пояснения» — период, часть, откуда каждая колонка. Лист длиннее предела строк Excel
//! продолжается на следующем («Тики (2)» …) — ничего не обрезается. Оформление — как «Итоги сделок» (results_sheet.rs:
//! шапка, Arial, закрепление первой строки, фильтр).
//! Кто вызывает: report/sig_parts.rs.
//! От чего зависит: rust_xlsxwriter, sig_sheet.rs (таблицы, продолжение листа), sig_model.rs, sig_pool.rs (fit).
//! Настройки: нет.

use rust_decimal::Decimal;
use rust_xlsxwriter::{Format, Workbook, XlsxError};

use super::sig_model::{BookRow, LiqRow, Sig, Tick, ts, ts_ms};
use super::sig_pool::fit;
use super::sig_sheet::{details, nums, put, table};

/// Колонки главного листа — дословно из задачи Тиграна 11.11 (тип биржи — «ЦЕКС» / «ДЕКС»).
pub const MAIN: [&str; 15] = [
    "Монета",
    "Смартконтракт",
    "Биржа 1",
    "Биржа 2",
    "Тип биржи 1",
    "Тип биржи 2",
    "Начало сигнального окна",
    "Конец сигнального окна",
    "Время трейда на бирже 1",
    "Время трейда на бирже 2",
    "Размер трейда на бирже 1",
    "Размер трейда на бирже 2",
    "Данные по тикам",
    "Данные по стакану",
    "Данные по ликвидности в пуле за окно",
];

/// Колонки объёмных листов.
pub const TICKS: [&str; 9] = [
    "Ключ сигнала",
    "Биржа",
    "Время сделки на бирже (UTC)",
    "Получено ботом (UTC)",
    "Цена",
    "Количество",
    "Сумма, USDT",
    "Сторона агрессора",
    "ID сделки",
];
pub const BOOKS: [&str; 12] = [
    "Ключ сигнала",
    "Биржа",
    "Источник",
    "Время биржи (UTC)",
    "Получено ботом (UTC)",
    "Лучший bid",
    "Лучший ask",
    "Уровней bid",
    "Уровней ask",
    "Bid всех уровней, USDT",
    "Ask всех уровней, USDT",
    "Пометка",
];
pub const LIQ: [&str; 13] = [
    "Ключ сигнала",
    "Время (UTC)",
    "Источник",
    "Направление",
    "Площадка",
    "Пул",
    "Вид пула",
    "Блок",
    "Сторона / объём",
    "Цена DEX, USDT",
    "Объём, монет",
    "Глубина, USDT",
    "Состояние пула",
];

/// Предел строк листа Excel — 1 048 576 («Total number of rows and columns on a worksheet»,
/// https://support.microsoft.com/office/excel-specifications-and-limits-1672b34d-7043-467e-8e27-269d656771c3).
pub const SHEET_MAX_ROWS: usize = 1_048_576;

/// Часть файла: сигналы и, у одного сигнала, доля объёмных строк (k из n).
#[derive(Debug, Clone)]
pub struct Part<'a> {
    pub sigs: Vec<&'a Sig>,
    pub cut: Option<(usize, usize)>,
}

/// Что делает: книга части. Принимает: часть, строки «Пояснений». Возвращает: байты .xlsx. Ошибки: XlsxError.
pub fn workbook(part: &Part<'_>, notes: &[String]) -> Result<Vec<u8>, XlsxError> {
    let mut wb = Workbook::new();
    let rows: Vec<_> = part.sigs.iter().flat_map(|s| &s.rows).collect();
    let wrap = Format::new().set_font_name("Arial").set_text_wrap();
    table(
        &mut wb,
        "Сигналы",
        &MAIN,
        &[
            10., 44., 10., 10., 9., 9., 23., 23., 34., 34., 40., 40., 60., 60., 60.,
        ],
        &rows,
        |ws, r, x| {
            let v = [
                &x.coin,
                &x.contract,
                &x.venue1,
                &x.venue2,
                &x.kind1,
                &x.kind2,
                &x.win_start,
                &x.win_end,
                &x.trade1_at,
                &x.trade2_at,
                &x.trade1_size,
                &x.trade2_size,
                &x.ticks,
                &x.book,
                &x.liquidity,
            ];
            for (c, t) in v.iter().enumerate() {
                ws.write_string_with_format(r, c as u16, fit(t), &wrap)?;
            }
            Ok(())
        },
    )?;
    let ticks: Vec<&Tick> = details(part, |s| &s.ticks);
    table(
        &mut wb,
        "Тики",
        &TICKS,
        &[24., 9., 24., 24., 14., 14., 14., 9., 18.],
        &ticks,
        |ws, r, x| {
            put(
                ws,
                r,
                &[
                    &x.key,
                    &x.venue,
                    &ts_ms(x.ex_ms),
                    &ts(crate::core::window_at::ns_time(x.recv_ns)),
                ],
            )?;
            nums(
                ws,
                r,
                4,
                &[Some(x.price), Some(x.qty), Some(x.price * x.qty)],
            )?;
            ws.write_string(r, 7, &x.taker)?;
            ws.write_string(r, 8, &x.id)?;
            Ok(())
        },
    )?;
    let books: Vec<&BookRow> = details(part, |s| &s.books);
    table(
        &mut wb,
        "Стаканы",
        &BOOKS,
        &[24., 9., 30., 24., 24., 14., 14., 9., 9., 16., 16., 30.],
        &books,
        |ws, r, x| {
            let ex = x.ex_ms.map(ts_ms).unwrap_or_default();
            put(ws, r, &[&x.key, &x.venue, &x.source, &ex, &ts(x.at)])?;
            nums(
                ws,
                r,
                5,
                &[
                    x.bid,
                    x.ask,
                    Some(x.levels.0.into()),
                    Some(x.levels.1.into()),
                    Some(x.usd.0),
                    Some(x.usd.1),
                ],
            )?;
            ws.write_string(r, 11, &x.note)?;
            Ok(())
        },
    )?;
    let liq: Vec<&LiqRow> = details(part, |s| &s.liq);
    table(
        &mut wb,
        "Ликвидность пула",
        &LIQ,
        &[
            24., 24., 26., 16., 9., 44., 22., 11., 30., 14., 14., 14., 80.,
        ],
        &liq,
        |ws, r, x| {
            let at = x.at.map(ts).unwrap_or_default();
            put(
                ws,
                r,
                &[
                    &x.key,
                    &at,
                    &x.source,
                    &x.direction,
                    &x.venue,
                    &x.pool,
                    &x.kind,
                ],
            )?;
            nums(ws, r, 7, &[x.block.map(Decimal::from)])?;
            ws.write_string(r, 8, &x.side)?;
            nums(ws, r, 9, &[x.price, x.qty, x.depth_usd])?;
            ws.write_string(r, 12, fit(&x.state))?;
            Ok(())
        },
    )?;
    let ws = wb.add_worksheet();
    ws.set_name("Пояснения")?;
    ws.set_column_width(0, 160.)?;
    for (i, t) in notes.iter().enumerate() {
        ws.write_string(i as u32, 0, fit(t))?;
    }
    wb.save_to_buffer()
}
