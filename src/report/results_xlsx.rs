//! Файл: report/results_xlsx.rs
//! Что делает: решения 193, 257 — книга «Итоги сделок» (arb_results_<дата>.xlsx): листы «Итог», «Сделки», «По дням»,
//! «Учёт дня», «По биржам», «По направлениям», «По монетам», «По часам UTC», «Слепок по балансам». Формулы считает Excel
//! / LibreOffice при открытии; все функции — из Excel 2007+ (решение 257: без MAXIFS / MINIFS). Оформление — как в
//! отчёте 05.11 (Arial, шапка белым по 1F4E78, примечания курсивом).
//! Кто вызывает: report/results.rs (results).
//! От чего зависит: rust_xlsxwriter, report/rows.rs, day_rows.rs, res_deals.rs, res_days.rs, res_ledger.rs,
//! res_summary.rs, results_sheet.rs, snap_sheet.rs, res_total.rs (лист «Итог»), rows_time.rs, core/daily_rows.rs (BotDay).
//! Настройки: нет.

use rust_xlsxwriter::{Workbook, XlsxError};

use crate::core::daily_rows::BotDay;

use super::day_rows::DayRow;
use super::res_deals::C;
use super::res_summary::summary;
use super::results_sheet::Key;
use super::rows::Row;
use super::rows_time::Window;
use super::snap::Snapshot;

/// Всё для книги.
pub struct Input<'a> {
    pub rows: &'a [Row],
    pub ledger: &'a [DayRow],
    pub bot: &'a [BotDay],
    pub snap: &'a Snapshot,
    /// Порог «Сделок дольше …», с (Err — причина).
    pub long_deal_s: &'a Result<u64, String>,
    pub window: Window,
    /// Что не удалось прочитать (итог дня бота и т. п.) — строкой в «Итог».
    pub problems: &'a [String],
}

/// Книга отчёта. Возвращает: байты .xlsx. Ошибки: XlsxError (запись книги).
pub fn workbook(x: &Input) -> Result<Vec<u8>, XlsxError> {
    let n = x.rows.len() as u32 + 1;
    let mut wb = Workbook::new();
    let at = x.window.now.format("%Y-%m-%d %H:%M:%S").to_string();
    wb.add_worksheet(); // «Итог» — первым листом, заполняется в конце (нужны строки «По дням» и «Слепка»).
    super::res_deals::sheet(wb.add_worksheet(), x.rows)?;
    let days = super::res_days::days(x.rows, x.ledger, x.bot, &x.window);
    super::res_days::by_day(
        wb.add_worksheet(),
        &days,
        x.bot,
        &x.window,
        x.ledger.len(),
        n,
    )?;
    super::res_ledger::ledger(wb.add_worksheet(), x.ledger)?;
    let venue_note = "Площадка итога — правило Тиграна 11.11 (решения 257 и 173 с дополнением t91): биржа ↔ DEX — бирже, биржа ↔ биржа — бирже покупки, DEX ↔ DEX — DEX; каждая сделка в одной строке, «Итого» = листу «Итог». Так же бот считает итог дня по площадкам и дневные лимиты.";
    let owners = by_count(x.rows.iter().map(|r| r.owner.clone()));
    summary(
        wb.add_worksheet(),
        "По биржам",
        "Площадка итога",
        &texts(&owners),
        C.owner,
        n,
        &[venue_note],
    )?;
    let dirs = by_count(x.rows.iter().map(|r| format!("{} → {}", r.buy, r.sell)));
    summary(
        wb.add_worksheet(),
        "По направлениям",
        "Направление",
        &texts(&dirs),
        C.dir,
        n,
        &[],
    )?;
    let note = "Отсортировано по итогу факт (на момент выгрузки).";
    summary(
        wb.add_worksheet(),
        "По монетам",
        "Монета",
        &texts(&coins(x.rows)),
        C.coin,
        n,
        &[note],
    )?;
    let hours: Vec<Key> = (0..24).map(Key::Num).collect();
    let note = "Час конца сделки (UTC).";
    summary(
        wb.add_worksheet(),
        "По часам UTC",
        "Час конца (UTC)",
        &hours,
        C.hour,
        n,
        &[note],
    )?;
    let snap_total = super::snap_sheet::sheet(wb.add_worksheet(), x.snap, &at)?;
    super::res_total::total(
        wb.worksheet_from_index(0)?,
        x,
        n,
        days.len() as u32 + 2,
        snap_total,
        &at,
    )?;
    wb.save_to_buffer()
}

fn texts(v: &[String]) -> Vec<Key> {
    v.iter().cloned().map(Key::Text).collect()
}

/// Значения по числу повторов, больше — выше (равные — по алфавиту). Ошибок нет.
fn by_count(it: impl Iterator<Item = String>) -> Vec<String> {
    let mut m: std::collections::BTreeMap<String, usize> = Default::default();
    for x in it {
        *m.entry(x).or_default() += 1;
    }
    let mut v: Vec<(String, usize)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().map(|(k, _)| k).collect()
}

/// Монеты по итогу факт, больше — выше (равные — по алфавиту). Ошибок нет.
fn coins(rows: &[Row]) -> Vec<String> {
    let mut m: std::collections::BTreeMap<String, f64> = Default::default();
    for r in rows {
        *m.entry(r.coin.clone()).or_default() += r.fact.unwrap_or(0.0);
    }
    let mut v: Vec<(String, f64)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().map(|(k, _)| k).collect()
}
