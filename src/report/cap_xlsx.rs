//! Файл: report/cap_xlsx.rs
//! Что делает: решение 256 (с дополнением — аудит отчёта, п. 2) — книга «Сделки, лимиты и капитал»
//! (arb_capital_<дата>[_part<K>].xlsx): листы «Сделки» (report/cap_deals.rs), «Упущено из-за потолка», «Итоги»
//! (формулы по первым двум; открывается первым), «Лимиты на момент выгрузки» (report/cap_limits.rs) и «Слепок по
//! балансам» — тот же лист и та же функция, что в «Итогах сделок» (report/snap_sheet.rs sheet по report/snap.rs gather).
//! Деление на файлы — report/cap_parts.rs (files).
//! Кто вызывает: report/cap_parts.rs, report/cap_send.rs, tests/report_capital.rs.
//! От чего зависит: rust_xlsxwriter, cap_deals.rs, cap_sheets.rs, cap_summary.rs, cap_limits.rs, cap_rows.rs,
//! cap_episodes.rs, snap.rs, snap_sheet.rs.
//! Настройки: нет.

use rust_decimal::Decimal;
use rust_xlsxwriter::{Workbook, XlsxError};

use super::cap_episodes::DayRow;
use super::cap_limits::LimitRow;
use super::cap_rows::DealRow;
use super::cap_sheets::{last_row, missed};
use super::cap_summary::{Ctx, summary};
use super::snap::Snapshot;

/// Данные книги.
#[derive(Clone)]
pub struct Data {
    pub deals: Vec<DealRow>,
    pub days: Vec<DayRow>,
    pub ladder: Vec<Decimal>,
    /// Заголовок листа «Итоги» (отчёт и период).
    pub title: String,
    /// Примечания листа «Итоги» (окно и время выгрузки, правила, причины без данных).
    pub notes: Vec<String>,
    pub limits: Vec<LimitRow>,
    /// Примечания листа «Лимиты».
    pub limit_notes: Vec<String>,
    pub snap: Snapshot,
    /// Время выгрузки текстом (UTC) — для листа «Слепок по балансам».
    pub at: String,
}

/// Готовый файл: имя, байты, подпись.
#[derive(Debug, Clone)]
pub struct File {
    pub name: String,
    pub bytes: Vec<u8>,
    pub caption: String,
}

/// Значения столбца без повторов по алфавиту. Ошибок нет.
fn uniq(v: impl Iterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = v.collect();
    v.sort();
    v.dedup();
    v
}

/// Направления периода по алфавиту (оба листа, без повторов). Ошибок нет.
pub fn dirs(data: &Data) -> Vec<String> {
    let deals = data.deals.iter().map(|r| r.dir.clone());
    uniq(deals.chain(data.days.iter().map(|r| r.dir.clone())))
}

/// Площадки денег покупки (сделки с покупкой) по алфавиту. Ошибок нет.
pub fn places(data: &Data) -> Vec<String> {
    uniq(
        data.deals
            .iter()
            .filter(|r| r.bought)
            .map(|r| r.hold.place.clone()),
    )
}

/// Книга по данным. Принимает: данные, доп. примечание части. Возвращает: байты .xlsx. Ошибки: XlsxError.
pub fn workbook(data: &Data, part: Option<&str>) -> Result<Vec<u8>, XlsxError> {
    let mut wb = Workbook::new();
    super::cap_deals::deals(wb.add_worksheet(), &data.deals)?;
    missed(wb.add_worksheet(), &data.days, &data.ladder)?;
    let (dirs, places) = (dirs(data), places(data));
    let mut notes = data.notes.clone();
    notes.extend(part.map(String::from));
    let cx = Ctx {
        dirs: &dirs,
        places: &places,
        deals_last: last_row(data.deals.len()),
        missed_last: last_row(data.days.len()),
        ladder: &data.ladder,
        title: data.title.clone(),
        notes,
    };
    let ws = wb.add_worksheet();
    summary(ws, &cx)?;
    ws.set_active(true);
    super::cap_limits::sheet(wb.add_worksheet(), &data.limits, &data.limit_notes)?;
    super::snap_sheet::sheet(wb.add_worksheet(), &data.snap, &data.at)?;
    wb.save_to_buffer()
}
