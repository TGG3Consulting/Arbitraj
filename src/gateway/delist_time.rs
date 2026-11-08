//! Файл: gateway/delist_time.rs
//! Что делает: строгий разбор даты и времени делистинга из английского текста анонса биржи (решение 226, уточнение
//! Тиграна 08.11.2026: «из анонса нужно получить ВРЕМЯ делистинга и блокировать только за delist.block_hours до
//! него»). Берётся только момент с датой, временем и явной пометкой UTC — дата без времени или без UTC не считается
//! (часовой пояс неизвестен — время «не определено», монета блокируется, решение 175).
//! Форматы — из живых ответов и страниц 08.11.2026 (tests/data/delist_ann_2026-11-08/):
//! - KuCoin: «8:00 on November 5, 2026 (UTC)»;
//! - Bybit: «Oct 28, 2026, 8:00AM UTC», заголовок «on 2026-09-04 08:00:00 UTC»;
//! - OKX: «October 3, 2026, 08:00 - 10:00 UTC» (окно — берётся начало), «8:00 am UTC on September 18, 2026»,
//!   «08:00 UTC on September 30, 2026»;
//! - Bitget: «November 13, 2026, 10:00 AM (UTC)»; MEXC: «Nov 9, 2026, 12:00 (UTC)».
//!
//! Кто вызывает: gateway/delist_ann.rs (заголовок и описание из API), gateway/delist_html.rs (страница анонса),
//! tests/delist_ann.rs.
//! От чего зависит: chrono.
//! Настройки: нет — это грамматика текста анонсов.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};

/// Часть текста: слово (латиница), число (цифры) или знак.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Num(String),
    Sym(char),
}

/// Месяцы: полные и короткие английские названия (как пишут биржи).
const MONTHS: [[&str; 3]; 12] = [
    ["January", "Jan", "Jan"],
    ["February", "Feb", "Feb"],
    ["March", "Mar", "Mar"],
    ["April", "Apr", "Apr"],
    ["May", "May", "May"],
    ["June", "Jun", "Jun"],
    ["July", "Jul", "Jul"],
    ["August", "Aug", "Aug"],
    ["September", "Sep", "Sept"],
    ["October", "Oct", "Oct"],
    ["November", "Nov", "Nov"],
    ["December", "Dec", "Dec"],
];

/// Разбивает текст на части; пробелы пропускаются. Ошибок нет.
fn tokens(text: &str) -> Vec<Tok> {
    let mut out = vec![];
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<Tok>| {
        if let Some(c) = cur.chars().next() {
            let t = std::mem::take(cur);
            out.push(if c.is_ascii_digit() {
                Tok::Num(t)
            } else {
                Tok::Word(t)
            });
        }
    };
    for c in text.chars() {
        let same = cur.chars().next().is_some_and(|p| {
            (p.is_ascii_digit() && c.is_ascii_digit())
                || (p.is_ascii_alphabetic() && c.is_ascii_alphabetic())
        });
        if !same {
            flush(&mut cur, &mut out);
        }
        if c.is_ascii_alphanumeric() {
            cur.push(c);
        } else if !c.is_whitespace() {
            out.push(Tok::Sym(c));
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Число из части. Принимает: части, позицию, допустимую длину цифр. Возвращает: число или None.
fn num(t: &[Tok], i: usize, len: std::ops::RangeInclusive<usize>) -> Option<u32> {
    match t.get(i) {
        Some(Tok::Num(s)) if len.contains(&s.len()) => s.parse().ok(),
        _ => None,
    }
}

/// Знак на позиции. Ошибок нет.
fn sym(t: &[Tok], i: usize, c: char) -> bool {
    t.get(i) == Some(&Tok::Sym(c))
}

/// Слово на позиции (без учёта регистра). Ошибок нет.
fn word(t: &[Tok], i: usize, w: &str) -> bool {
    matches!(t.get(i), Some(Tok::Word(s)) if s.eq_ignore_ascii_case(w))
}

/// Дата с позиции: «November 5, 2026» или «2026-09-04». Возвращает: (дата, следующая позиция) или None.
fn date(t: &[Tok], i: usize) -> Option<(NaiveDate, usize)> {
    if let Some(Tok::Word(w)) = t.get(i) {
        let m = MONTHS.iter().position(|m| m.contains(&w.as_str()))? as u32 + 1;
        let d = num(t, i + 1, 1..=2)?;
        if !sym(t, i + 2, ',') {
            return None;
        }
        let y = num(t, i + 3, 4..=4)?;
        return NaiveDate::from_ymd_opt(y as i32, m, d).map(|x| (x, i + 4));
    }
    let y = num(t, i, 4..=4)?;
    if !(sym(t, i + 1, '-') && sym(t, i + 3, '-')) {
        return None;
    }
    let (m, d) = (num(t, i + 2, 2..=2)?, num(t, i + 4, 2..=2)?);
    NaiveDate::from_ymd_opt(y as i32, m, d).map(|x| (x, i + 5))
}

/// Время с позиции: «8:00», «08:00:00», «8:00AM», «10:00 am». Возвращает: (секунды от полуночи, следующая
/// позиция) или None.
fn clock(t: &[Tok], i: usize) -> Option<(u32, usize)> {
    let h = num(t, i, 1..=2)?;
    if !sym(t, i + 1, ':') {
        return None;
    }
    let mi = num(t, i + 2, 2..=2)?;
    let mut j = i + 3;
    let mut s = 0;
    if sym(t, j, ':') {
        s = num(t, j + 1, 2..=2)?;
        j += 2;
    }
    let mut h = h;
    if word(t, j, "am") || word(t, j, "pm") {
        if !(1..=12).contains(&h) {
            return None;
        }
        let pm = word(t, j, "pm");
        h = h % 12 + if pm { 12 } else { 0 };
        j += 1;
    }
    (h < 24 && mi < 60 && s < 60).then_some((h * 3600 + mi * 60 + s, j))
}

/// Пометка UTC с позиции: «UTC» или «(UTC)». Возвращает: следующая позиция или None.
fn utc(t: &[Tok], i: usize) -> Option<usize> {
    if word(t, i, "UTC") {
        return Some(i + 1);
    }
    (sym(t, i, '(') && word(t, i + 1, "UTC") && sym(t, i + 2, ')')).then_some(i + 3)
}

/// Момент из даты и секунд. Ошибок нет (None — вне диапазона).
fn at(d: NaiveDate, secs: u32) -> Option<DateTime<Utc>> {
    let n = d.and_hms_opt(secs / 3600, secs / 60 % 60, secs % 60)?;
    Utc.from_local_datetime(&n).single()
}

/// Что делает: все моменты «дата + время + UTC» в тексте, по порядку. Принимает: текст. Возвращает: моменты
/// (повторы не убираются). Ошибок нет.
pub fn all(text: &str) -> Vec<DateTime<Utc>> {
    let t = tokens(text);
    let mut out = vec![];
    let mut i = 0;
    while i < t.len() {
        // Дата, затем время: «Oct 28, 2026, 8:00AM UTC», «October 3, 2026, 08:00 - 10:00 UTC».
        if let Some((d, j)) = date(&t, i) {
            let j = if sym(&t, j, ',') { j + 1 } else { j };
            if let Some((secs, mut k)) = clock(&t, j) {
                if sym(&t, k, '-')
                    && let Some((_, e)) = clock(&t, k + 1)
                {
                    k = e;
                }
                if let (Some(e), Some(m)) = (utc(&t, k), at(d, secs)) {
                    out.push(m);
                    i = e;
                    continue;
                }
            }
        }
        // Время, затем дата: «8:00 on November 5, 2026 (UTC)», «8:00 am UTC on September 18, 2026».
        if let Some((secs, j)) = clock(&t, i) {
            let (u1, j) = utc(&t, j).map_or((false, j), |e| (true, e));
            if word(&t, j, "on")
                && let Some((d, k)) = date(&t, j + 1)
            {
                let (u2, k) = utc(&t, k).map_or((false, k), |e| (true, e));
                if let (true, Some(m)) = (u1 || u2, at(d, secs)) {
                    out.push(m);
                    i = k;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Что делает: единственный момент в тексте. Принимает: текст. Возвращает: Some — в тексте ровно один различный
/// момент «дата + время + UTC»; None — ни одного или несколько разных (время не определено однозначно). Ошибок нет.
pub fn one(text: &str) -> Option<DateTime<Utc>> {
    distinct(all(text))
}

/// Что делает: единственный различный момент из списка. Принимает: моменты. Возвращает: Some — все равны (и список
/// не пуст); иначе None. Ошибок нет.
pub fn distinct(list: Vec<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    let first = *list.first()?;
    list.iter().all(|m| *m == first).then_some(first)
}
