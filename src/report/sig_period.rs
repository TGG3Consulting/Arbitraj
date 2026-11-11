//! Файл: report/sig_period.rs
//! Что делает: решение 255 (Тигран 11.11) — периоды отчёта «Сигналы и сделки»: за всё время, текущий день, 3 дня,
//! 5 дней, 7 дней, свой интервал. Часовой пояс — UTC, как у существующих отчётов (report/mod.rs period_text, rows.rs —
//! время в UTC). «Текущий день» — с 00:00 UTC сегодня до момента выгрузки; «N дней» — последние N × 24 ч до момента
//! выгрузки, как «7 дней» решения 215 (notify/ctl/report.rs PERIODS); свой интервал — строго «дд.мм.гг – дд.мм.гг»
//! (обе даты включительно: с 00:00 UTC первой по 24:00 UTC второй), тире — «–», «—» или «-», пробелы вокруг — любые.
//! Кто вызывает: notify/ctl/report_sig.rs, notify/ctl/report_cap.rs (решение 256 — те же периоды), тесты
//! (tests/report_signals.rs, tests/report_capital.rs).
//! От чего зависит: chrono.
//! Настройки: нет (периоды — кнопки, заданные Тиграном 11.11, а не пороги).

use chrono::{DateTime, Duration, NaiveDate, Utc};

/// Период отчёта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    All,
    Today,
    /// Последние N × 24 ч.
    Days(i64),
    /// Свой интервал: дни включительно (UTC).
    Range(NaiveDate, NaiveDate),
}

/// Кнопки периодов: код callback_data, надпись, период (свой интервал — код «c», ввод текстом).
pub const BUTTONS: [(&str, &str, Option<Period>); 6] = [
    ("a", "За всё время", Some(Period::All)),
    ("t", "Текущий день", Some(Period::Today)),
    ("3", "3 дня", Some(Period::Days(3))),
    ("5", "5 дней", Some(Period::Days(5))),
    ("7", "7 дней", Some(Period::Days(7))),
    ("c", "Свой интервал", None),
];

impl Period {
    /// Что делает: границы [from, to) в UTC. Принимает: момент выгрузки. Возвращает: (from, to); None — без границы.
    /// Ошибок нет.
    pub fn bounds(&self, now: DateTime<Utc>) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
        let midnight = |d: NaiveDate| d.and_hms_opt(0, 0, 0).map(|t| t.and_utc());
        match *self {
            Period::All => (None, None),
            Period::Today => (midnight(now.date_naive()), None),
            Period::Days(n) => (Some(now - Duration::days(n)), None),
            Period::Range(a, b) => (midnight(a), b.succ_opt().and_then(midnight)),
        }
    }

    /// Надпись периода для подписи файла. Ошибок нет.
    pub fn label(&self) -> String {
        match self {
            Period::All => "за всё время".into(),
            Period::Today => "текущий день (UTC)".into(),
            Period::Days(n) => format!("{n} дн."),
            Period::Range(a, b) => format!("{} – {}", a.format("%d.%m.%y"), b.format("%d.%m.%y")),
        }
    }
}

/// Что делает: разбирает свой интервал «дд.мм.гг – дд.мм.гг» (см. шапку). Принимает: текст человека.
/// Возвращает: Period::Range. Ошибки: текст для человека — что не так и как надо.
pub fn parse_range(text: &str) -> Result<Period, String> {
    let how = "нужно две даты дд.мм.гг через тире, например 01.11.26 – 07.11.26";
    let t = text.trim();
    let parts: Vec<&str> = t.split(['–', '—', '-']).map(str::trim).collect();
    let [a, b] = parts.as_slice() else {
        return Err(format!("«{t}» — не интервал: {how}"));
    };
    let (a, b) = (date(a, how)?, date(b, how)?);
    if a > b {
        return Err(format!(
            "начало {} позже конца {} — {how}",
            a.format("%d.%m.%y"),
            b.format("%d.%m.%y")
        ));
    }
    Ok(Period::Range(a, b))
}

/// Одна дата строго дд.мм.гг (две цифры дня, месяца и года; год — 20гг). Ошибки: текст для человека.
fn date(s: &str, how: &str) -> Result<NaiveDate, String> {
    let bad = || format!("«{s}» — не дата дд.мм.гг: {how}");
    let f: Vec<&str> = s.split('.').collect();
    let ok = f.len() == 3
        && f.iter()
            .all(|x| x.len() == 2 && x.bytes().all(|c| c.is_ascii_digit()));
    if !ok {
        return Err(bad());
    }
    let n = |x: &str| x.parse::<u32>().map_err(|_| bad());
    let (d, m, y) = (n(f[0])?, n(f[1])?, n(f[2])?);
    NaiveDate::from_ymd_opt(2000 + y as i32, m, d)
        .ok_or_else(|| format!("«{s}» — такой даты нет: {how}"))
}
