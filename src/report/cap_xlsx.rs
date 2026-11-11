//! Файл: report/cap_xlsx.rs
//! Что делает: решение 256 — книга «Сделки, лимиты и капитал» (arb_capital_<дата>[_partKofN].xlsx): листы «Сделки»,
//! «Упущено из-за потолка», «Итоги» (формулы по первым двум; лист «Итоги» открывается первым), и деление на файлы не
//! больше предела отправки. Предел передаёт вызывающий: для Telegram — notify/tg_methods.rs DOCUMENT_MAX_BYTES
//! (sendDocument, «50 MB», https://core.telegram.org/bots/api#senddocument). Правило деления: книга целиком; больше
//! предела — дни периода (дни начала сделок и эпизодов, UTC) делятся пополам, каждая часть — полная книга по своим
//! дням (формулы «Итогов» считают только свою часть; сделки, начатые в другой части, в «одновременно открыто» этой
//! части не входят — сказано в примечании части); один день больше предела — ошибка с причиной, ничего не
//! отбрасывается молча.
//! Кто вызывает: report/mod.rs (capital_send), tests/report_capital.rs.
//! От чего зависит: rust_xlsxwriter, cap_sheets.rs, cap_summary.rs, cap_rows.rs, cap_episodes.rs.
//! Настройки: нет.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use rust_xlsxwriter::{Workbook, XlsxError};

use super::cap_episodes::DayRow;
use super::cap_rows::DealRow;
use super::cap_sheets::{deals, last_row, missed};
use super::cap_summary::{Ctx, summary};

/// Данные книги.
pub struct Data {
    pub deals: Vec<DealRow>,
    pub days: Vec<DayRow>,
    pub ladder: Vec<Decimal>,
    /// Заголовок листа «Итоги» (отчёт и период).
    pub title: String,
    /// Примечания листа «Итоги» (время выгрузки, правила, причины без данных).
    pub notes: Vec<String>,
}

/// Готовый файл: имя, байты, подпись.
#[derive(Debug, Clone)]
pub struct File {
    pub name: String,
    pub bytes: Vec<u8>,
    pub caption: String,
}

/// Направления периода по алфавиту (оба листа, без повторов). Ошибок нет.
pub fn dirs(data: &Data) -> Vec<String> {
    let mut v: Vec<String> = data
        .deals
        .iter()
        .map(|r| r.dir.clone())
        .chain(data.days.iter().map(|r| r.dir.clone()))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Книга по данным. Принимает: данные, доп. примечание части. Возвращает: байты .xlsx. Ошибки: XlsxError.
pub fn workbook(data: &Data, part: Option<&str>) -> Result<Vec<u8>, XlsxError> {
    let mut wb = Workbook::new();
    deals(wb.add_worksheet(), &data.deals)?;
    missed(wb.add_worksheet(), &data.days, &data.ladder)?;
    let dirs = dirs(data);
    let mut notes = data.notes.clone();
    notes.extend(part.map(String::from));
    let cx = Ctx {
        dirs: &dirs,
        deals_last: last_row(data.deals.len()),
        missed_last: last_row(data.days.len()),
        ladder: &data.ladder,
        title: data.title.clone(),
        notes,
    };
    let ws = wb.add_worksheet();
    summary(ws, &cx)?;
    ws.set_active(true);
    wb.save_to_buffer()
}

/// Дни данных по возрастанию (начало сделок и эпизодов, UTC). Ошибок нет.
pub fn days(data: &Data) -> Vec<NaiveDate> {
    let mut v: Vec<NaiveDate> = data
        .deals
        .iter()
        .map(|r| r.start.date_naive())
        .chain(data.days.iter().map(|r| r.day))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Данные только за дни [a, b] включительно. Ошибок нет.
fn slice(data: &Data, a: NaiveDate, b: NaiveDate) -> Data {
    let inside = |d: NaiveDate| d >= a && d <= b;
    Data {
        deals: (data.deals.iter())
            .filter(|r| inside(r.start.date_naive()))
            .cloned()
            .collect(),
        days: data
            .days
            .iter()
            .filter(|r| inside(r.day))
            .cloned()
            .collect(),
        ladder: data.ladder.clone(),
        title: data.title.clone(),
        notes: data.notes.clone(),
    }
}

/// Подпись дней части. Ошибок нет.
fn span(a: NaiveDate, b: NaiveDate) -> String {
    let f = |d: NaiveDate| d.format("%d.%m.%y").to_string();
    if a == b {
        f(a)
    } else {
        format!("{} – {}", f(a), f(b))
    }
}

/// Что делает: файлы книги (см. шапку). Принимает: данные, предел байт, префикс имени без «.xlsx», подпись, `emit` —
/// куда отдать готовый файл (false — получатель закрыт: сборка останавливается). Возвращает: сколько файлов отдано.
/// Ошибки: текст — книга не собралась или один день больше предела.
pub fn files(
    data: &Data,
    max: usize,
    prefix: &str,
    caption: &str,
    emit: &mut dyn FnMut(File) -> bool,
) -> Result<usize, String> {
    let whole = workbook(data, None).map_err(|e| format!("отчёт xlsx: {e}"))?;
    if whole.len() <= max {
        emit(File {
            name: format!("{prefix}.xlsx"),
            bytes: whole,
            caption: caption.into(),
        });
        return Ok(1);
    }
    drop(whole);
    // Части по дням: сначала делим, пока каждая не влезет, потом отдаём по порядку (число частей — в имени).
    let all = days(data);
    let mut parts: Vec<(usize, usize, Vec<u8>)> = vec![];
    split(data, &all, 0, all.len(), max, &mut parts)?;
    let n = parts.len();
    for (k, (i, j, bytes)) in parts.into_iter().enumerate() {
        let s = span(all[i], all[j - 1]);
        let f = File {
            name: format!("{prefix}_part{}of{n}.xlsx", k + 1),
            bytes,
            caption: format!("{caption}; часть {} из {n}: дни {s}", k + 1),
        };
        if !emit(f) {
            return Ok(k);
        }
    }
    Ok(n)
}

/// Собирает часть дней [i, j); больше предела — делит пополам. Ошибки: текст.
fn split(
    data: &Data,
    all: &[NaiveDate],
    i: usize,
    j: usize,
    max: usize,
    out: &mut Vec<(usize, usize, Vec<u8>)>,
) -> Result<(), String> {
    let (a, b) = (all[i], all[j - 1]);
    let note = format!(
        "Часть: дни {} (UTC). Формулы считают только эту часть; сделки других частей в «одновременно открыто» не входят.",
        span(a, b)
    );
    let bytes =
        workbook(&slice(data, a, b), Some(&note)).map_err(|e| format!("отчёт xlsx: {e}"))?;
    if bytes.len() <= max {
        out.push((i, j, bytes));
        return Ok(());
    }
    if j - i == 1 {
        return Err(format!(
            "день {} — файл {} байт больше предела {max} байт, не делится дальше (по дням)",
            span(a, b),
            bytes.len()
        ));
    }
    drop(bytes);
    let mid = i + (j - i) / 2;
    split(data, all, i, mid, max, out)?;
    split(data, all, mid, j, max, out)
}
