//! Файл: report/cap_parts.rs
//! Что делает: решение 256 (с дополнением — аудит отчёта, п. 10) — деление книги «Сделки, лимиты и капитал»
//! (report/cap_xlsx.rs workbook) на файлы не больше предела отправки (для Telegram — notify/tg_methods.rs
//! DOCUMENT_MAX_BYTES, sendDocument «50 MB», https://core.telegram.org/bots/api#senddocument): книга целиком; больше
//! предела — дни данных (день начала сделки и эпизода, UTC) делятся пополам, каждая часть — полная книга своих дней.
//! Часть отдаётся СРАЗУ после сборки — в памяти не копятся все части (обещано в решении 256 (3)); имя _part<K> по
//! порядку дней, как у «Сигналов и сделок» (решение 255: всего частей заранее не известно — итог «файлов N» пишет
//! вызывающий). Каждая строка — ровно в одной части (по дню начала); формулы «Итогов» и занятость считают только свою
//! часть — сказано в примечании части; один день больше предела — ошибка с причиной.
//! Кто вызывает: report/cap_send.rs (capital_send), tests/report_capital.rs.
//! От чего зависит: report/cap_xlsx.rs (Data, File, workbook).
//! Настройки: нет.

use chrono::NaiveDate;

use super::cap_xlsx::{Data, File, workbook};

/// Дни данных по возрастанию (начало сделок и эпизодов, UTC). Ошибок нет.
pub fn days(data: &Data) -> Vec<NaiveDate> {
    let deals = data.deals.iter().map(|r| r.start.date_naive());
    let mut v: Vec<NaiveDate> = deals.chain(data.days.iter().map(|r| r.day)).collect();
    v.sort();
    v.dedup();
    v
}

/// Данные только за дни [a, b] включительно. Ошибок нет.
pub fn slice(data: &Data, a: NaiveDate, b: NaiveDate) -> Data {
    let inside = |d: NaiveDate| d >= a && d <= b;
    Data {
        deals: (data.deals.iter())
            .filter(|r| inside(r.start.date_naive()))
            .cloned()
            .collect(),
        days: (data.days.iter())
            .filter(|r| inside(r.day))
            .cloned()
            .collect(),
        ..data.clone_head()
    }
}

impl Data {
    /// Всё, кроме строк сделок и эпизодов (для части). Ошибок нет.
    fn clone_head(&self) -> Data {
        Data {
            deals: vec![],
            days: vec![],
            ladder: self.ladder.clone(),
            title: self.title.clone(),
            notes: self.notes.clone(),
            limits: self.limits.clone(),
            limit_notes: self.limit_notes.clone(),
            snap: self.snap.clone(),
            at: self.at.clone(),
        }
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

/// Куда отдавать части: предел, имя, подпись, получатель, счётчик отданных.
struct Out<'a> {
    max: usize,
    prefix: &'a str,
    caption: &'a str,
    emit: &'a mut dyn FnMut(File) -> bool,
    sent: usize,
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
        let f = File {
            name: format!("{prefix}.xlsx"),
            bytes: whole,
            caption: caption.into(),
        };
        return Ok(usize::from(emit(f)));
    }
    drop(whole);
    let all = days(data);
    let mut out = Out {
        max,
        prefix,
        caption,
        emit,
        sent: 0,
    };
    if all.len() > 1 {
        let mid = all.len() / 2;
        if split(data, &all, 0, mid, &mut out)? {
            split(data, &all, mid, all.len(), &mut out)?;
        }
        return Ok(out.sent);
    }
    let s = all.first().map(|d| span(*d, *d)).unwrap_or_default();
    Err(format!(
        "день {s} — файл больше предела {max} байт, не делится дальше (по дням)"
    ))
}

/// Собирает часть дней [i, j) и сразу отдаёт; больше предела — делит пополам. Возвращает: false — получатель закрыт.
/// Ошибки: текст.
fn split(data: &Data, all: &[NaiveDate], i: usize, j: usize, o: &mut Out) -> Result<bool, String> {
    let (a, b) = (all[i], all[j - 1]);
    let note = format!(
        "Часть: дни {} (UTC, по началу сделки и эпизода). Формулы считают только эту часть; сделки других частей в \
         занятость и итог этой части не входят.",
        span(a, b)
    );
    let bytes =
        workbook(&slice(data, a, b), Some(&note)).map_err(|e| format!("отчёт xlsx: {e}"))?;
    if bytes.len() <= o.max {
        let k = o.sent + 1;
        let f = File {
            name: format!("{}_part{k}.xlsx", o.prefix),
            bytes,
            caption: format!("{}; часть {k}: дни {}", o.caption, span(a, b)),
        };
        if !(o.emit)(f) {
            return Ok(false);
        }
        o.sent = k;
        return Ok(true);
    }
    if j - i == 1 {
        return Err(format!(
            "день {} — файл {} байт больше предела {} байт, не делится дальше (по дням)",
            span(a, b),
            bytes.len(),
            o.max
        ));
    }
    drop(bytes);
    let mid = i + (j - i) / 2;
    Ok(split(data, all, i, mid, o)? && split(data, all, mid, j, o)?)
}
