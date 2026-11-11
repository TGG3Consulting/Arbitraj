//! Файл: report/sig_parts.rs
//! Что делает: решение 255 — деление книги «Сигналы и сделки» на файлы не больше предела отправки. Предел передаёт
//! вызывающий: для Telegram — notify/tg_methods.rs DOCUMENT_MAX_BYTES (sendDocument: «Bots can currently send files of
//! any type of up to 50 MB in size», https://core.telegram.org/bots/api#senddocument). Почему деление, а не zip: файл
//! .xlsx — уже архив zip (сжатие deflate), повторное сжатие почти ничего не даёт, а архив больше предела всё равно не
//! отправить. Правило: книга целиком; больше предела — сигналы делятся пополам по времени (части идут по датам, каждый
//! сигнал целиком в одной части); один сигнал больше предела — его объёмные строки («Тики», «Стаканы», «Ликвидность
//! пула») делятся на доли по порядку времени (строка главного листа повторяется в каждой доле, в «Пояснениях» — «доля
//! k из n»). Ничего не отбрасывается молча: если даже доля в одну строку на лист больше предела — ошибка с причиной.
//! Кто вызывает: report/sig_stream.rs (fit, describe — отчёт потоком), тесты (tests/report_signals.rs: files).
//! От чего зависит: sig_xlsx.rs, sig_model.rs.
//! Настройки: нет.

use super::sig_model::{Sig, ts};
use super::sig_xlsx::{Part, workbook};

/// Готовый файл: имя, байты, подпись.
#[derive(Debug, Clone)]
pub struct File {
    pub name: String,
    pub bytes: Vec<u8>,
    pub caption: String,
}

/// Что делает: файлы книги (см. шапку). Принимает: сигналы (старые сверху), предел байт, префикс имени без «.xlsx»,
/// подпись, строки «Пояснений». Возвращает: файлы по порядку времени. Ошибки: текст — книга не собралась или не делится.
pub fn files(
    sigs: &[Sig],
    max: usize,
    prefix: &str,
    caption: &str,
    notes: &[String],
) -> Result<Vec<File>, String> {
    let mut done: Vec<(Part<'_>, Vec<u8>)> = vec![];
    let all = Part {
        sigs: sigs.iter().collect(),
        cut: None,
    };
    fit(all, max, notes, &mut done)?;
    let n = done.len();
    Ok(done
        .into_iter()
        .enumerate()
        .map(|(i, (p, bytes))| {
            if n == 1 {
                return File {
                    name: format!("{prefix}.xlsx"),
                    bytes,
                    caption: caption.into(),
                };
            }
            File {
                name: format!("{prefix}_part{}of{n}.xlsx", i + 1),
                bytes,
                caption: format!("{caption}; часть {} из {n}: {}", i + 1, describe(&p)),
            }
        })
        .collect())
}

/// Собирает часть; больше предела — делит (см. шапку) и собирает доли по порядку. Ошибки: текст.
pub fn fit<'a>(
    p: Part<'a>,
    max: usize,
    notes: &[String],
    out: &mut Vec<(Part<'a>, Vec<u8>)>,
) -> Result<(), String> {
    let mut n = notes.to_vec();
    n.push(format!("Эта часть: {}", describe(&p)));
    let bytes = workbook(&p, &n).map_err(|e| format!("отчёт xlsx: {e}"))?;
    if bytes.len() <= max {
        out.push((p, bytes));
        return Ok(());
    }
    if p.sigs.len() > 1 {
        let mid = p.sigs.len() / 2;
        let (a, b) = p.sigs.split_at(mid);
        fit(
            Part {
                sigs: a.to_vec(),
                cut: None,
            },
            max,
            notes,
            out,
        )?;
        return fit(
            Part {
                sigs: b.to_vec(),
                cut: None,
            },
            max,
            notes,
            out,
        );
    }
    let (k, n) = p.cut.unwrap_or((0, 1));
    let longest = p
        .sigs
        .iter()
        .map(|s| s.ticks.len().max(s.books.len()).max(s.liq.len()))
        .max()
        .unwrap_or(0);
    if p.sigs.is_empty() || n >= longest.max(1) {
        return Err(format!(
            "файл {} байт больше предела {max} байт и дальше не делится ({})",
            bytes.len(),
            describe(&p)
        ));
    }
    for k2 in [2 * k, 2 * k + 1] {
        fit(
            Part {
                sigs: p.sigs.clone(),
                cut: Some((k2, 2 * n)),
            },
            max,
            notes,
            out,
        )?;
    }
    Ok(())
}

/// Описание части: сигналы с … по …, доля. Ошибок нет.
pub fn describe(p: &Part<'_>) -> String {
    let (Some(a), Some(b)) = (p.sigs.first(), p.sigs.last()) else {
        return "сигналов нет".into();
    };
    let mut s = format!(
        "сигналов {}, с {} по {} UTC",
        p.sigs.len(),
        ts(a.at),
        ts(b.at)
    );
    if let Some((k, n)) = p.cut {
        s.push_str(&format!(
            ", объёмные строки сигнала {} — доля {} из {n}",
            a.key,
            k + 1
        ));
    }
    s
}
