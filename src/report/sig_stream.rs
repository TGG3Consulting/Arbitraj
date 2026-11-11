//! Файл: report/sig_stream.rs
//! Что делает: решение 255 — отчёт «Сигналы и сделки» потоком, чтобы память не росла с периодом: сигналы читаются по
//! одному (sig_build.rs make — файл окна и эпизоды), копятся в пачку, пока ни один объёмный лист пачки не дошёл до
//! предела строк листа Excel (sig_xlsx.rs SHEET_MAX_ROWS — граница пачки из формата файла, а не порог); пачка → книги
//! не больше предела байт (sig_parts.rs fit: деление по сигналам, у огромного сигнала — доли строк) → файлы отдаются
//! вызывающему по одному (emit) и забываются. Один файл держится до следующего, чтобы назвать единственный файл без
//! номера части: один — arb_signals_<дата>.xlsx, несколько — arb_signals_<дата>_part<K>.xlsx (сколько всего — заранее
//! неизвестно; итог — возвращаемое число). Блокирующее — вызывать в spawn_blocking.
//! Кто вызывает: report/mod.rs (signals_stream).
//! От чего зависит: core/signals_src.rs, sig_build.rs, sig_link.rs (link), sig_parts.rs, sig_xlsx.rs, sig_model.rs.
//! Настройки: нет.

use chrono::{DateTime, Utc};

use crate::core::signals_src::SignalsSrc;

use super::sig_model::Sig;
use super::sig_parts::{File, describe, fit};
use super::sig_xlsx::{Part, SHEET_MAX_ROWS};

/// Готовая книга, ещё не отданная: описание части и байты.
type Pending = (String, Vec<u8>);

/// Что делает: файлы отчёта за период по одному в `emit` (см. шапку). Принимает: данные, границы, предел байт файла,
/// префикс имени без «.xlsx», подпись, строки «Пояснений», приёмник файла (false — дальше не нужно).
/// Возвращает: сколько файлов отдано. Ошибки: текст — книга не собралась, не делится до предела, приёмник отказался.
#[allow(clippy::too_many_arguments)]
pub fn stream(
    src: &SignalsSrc,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    max: usize,
    prefix: &str,
    caption: &str,
    notes: &[String],
    emit: &mut dyn FnMut(File) -> bool,
) -> Result<usize, String> {
    let links = super::sig_link::link(src);
    let mut st = State {
        pending: None,
        sent: 0,
        prefix,
        caption,
    };
    let mut batch: Vec<Sig> = vec![];
    for (_, item) in super::sig_build::items(src, &links, from, to) {
        batch.push(super::sig_build::make(src, &links, &item));
        let full = |f: fn(&Sig) -> usize| batch.iter().map(f).sum::<usize>() >= SHEET_MAX_ROWS - 1;
        if full(|s| s.ticks.len()) || full(|s| s.books.len()) || full(|s| s.liq.len()) {
            st.flush(&std::mem::take(&mut batch), max, notes, emit)?;
        }
    }
    if !batch.is_empty() || (st.pending.is_none() && st.sent == 0) {
        st.flush(&batch, max, notes, emit)?;
    }
    st.finish(emit)
}

/// Ход отдачи файлов.
struct State<'a> {
    pending: Option<Pending>,
    sent: usize,
    prefix: &'a str,
    caption: &'a str,
}

impl State<'_> {
    /// Книги пачки (с делением до предела); каждая предыдущая отдаётся как часть. Ошибки: текст.
    fn flush(
        &mut self,
        batch: &[Sig],
        max: usize,
        notes: &[String],
        emit: &mut dyn FnMut(File) -> bool,
    ) -> Result<(), String> {
        let mut out = vec![];
        fit(
            Part {
                sigs: batch.iter().collect(),
                cut: None,
            },
            max,
            notes,
            &mut out,
        )?;
        for (p, bytes) in out {
            if let Some(prev) = self.pending.take() {
                self.give(prev, emit)?;
            }
            self.pending = Some((describe(&p), bytes));
        }
        Ok(())
    }

    /// Отдаёт часть с номером. Ошибки: приёмник отказался.
    fn give(
        &mut self,
        (what, bytes): Pending,
        emit: &mut dyn FnMut(File) -> bool,
    ) -> Result<(), String> {
        self.sent += 1;
        let f = File {
            name: format!("{}_part{}.xlsx", self.prefix, self.sent),
            bytes,
            caption: format!("{}; часть {}: {what}", self.caption, self.sent),
        };
        if emit(f) {
            Ok(())
        } else {
            Err(format!(
                "отдача файлов прервана после части {}",
                self.sent - 1
            ))
        }
    }

    /// Последний файл: единственный — без номера части, иначе — следующая часть. Возвращает: сколько отдано.
    /// Ошибки: приёмник отказался.
    fn finish(mut self, emit: &mut dyn FnMut(File) -> bool) -> Result<usize, String> {
        let Some((what, bytes)) = self.pending.take() else {
            return Ok(self.sent);
        };
        if self.sent > 0 {
            self.give((what, bytes), emit)?;
            return Ok(self.sent);
        }
        let f = File {
            name: format!("{}.xlsx", self.prefix),
            bytes,
            caption: self.caption.to_string(),
        };
        if !emit(f) {
            return Err("отдача файла прервана".into());
        }
        Ok(1)
    }
}
