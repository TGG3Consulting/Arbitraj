//! Файл: notify/reason.rs
//! Что делает: человеческая причина из текста ошибки (решение 164). Ошибки бирж приходят от адаптеров в виде
//! «MEXC, код 16021: <msg>» и «Gate, HTTP 400, TOO_FAST: <msg>» (venues/*/errors.rs, from_code / from_label): известный
//! код — человеческая формулировка из словаря notify/reason_codes.rs, неизвестный — короткое «Ответ биржи: код и текст
//! как есть» (обрезано). Внутренние слова (Halted, Stopped…) и ссылки на номера решений из текста убираются.
//! Логику не меняет: исходный текст остаётся в журнале и в хронологии сделки.
//! Кто вызывает: notify/human.rs (Msg::reason), ядро (короткая причина для экрана «Ручное управление»).
//! От чего зависит: notify/reason_codes.rs, notify/human.rs (имя площадки).
//! Настройки: нет.

use super::reason_codes::lookup;

/// Предел длины причины и ответа биржи (символов) — чтобы читалось с телефона.
const MAX_REASON: usize = 160;
const MAX_ANSWER: usize = 120;

/// Причина для человека.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    /// Причина понятными словами.
    pub text: String,
    /// Неизвестная ошибка биржи — её код и текст как есть (обрезано), иначе None.
    pub answer: Option<String>,
}

/// Как площадки подписывают ошибку с кодом (venues/*/errors.rs): «<имя>, код <код>: <msg>».
const CODE_PREFIXES: [(&str, &str); 4] = [
    ("BingX, код ", "bingx"),
    ("MEXC, код ", "mexc"),
    ("Bybit, код ", "bybit"),
    ("KuCoin, код ", "kucoin"),
];
/// Gate: «Gate, HTTP <статус>, <label>: <message>» (venues/gate/errors.rs, from_label).
const GATE_PREFIX: &str = "Gate, HTTP ";

/// Служебные приставки в начале текста: Step в Debug-виде (старые записи хронологии) и приставки хронологии сделки
/// «Сделка остановлена: » (core/ledger.rs) — в причине они лишние.
const PREFIXES: [&str; 8] = [
    "Halted: ",
    "Closed: ",
    "TakenOver: ",
    "Stopped ",
    "Сделка остановлена: ",
    "Сделка закрыта: ",
    "Сделка монеты у человека: ",
    "Сделка ждёт решения человека: ",
];

/// Человеческая причина. Принимает: исходный текст ошибки. Возвращает: Reason. Ошибок нет.
pub fn human(raw: &str) -> Reason {
    let s = clean(raw);
    let Some((pos, venue, code, msg)) = find_code(&s) else {
        return Reason {
            text: cut(&s, MAX_REASON),
            answer: None,
        };
    };
    let ctx = s[..pos]
        .trim_end_matches([' ', ':', '—', '(', ','])
        .to_string();
    let name = super::human::venue(venue);
    match lookup(venue, &code) {
        Some(h) => Reason {
            // Формулировка уже начинается с имени площадки («Gate не разрешает…») — имя не повторяем.
            text: cap(&join(
                &ctx,
                &if h.starts_with(&name) {
                    h.to_string()
                } else {
                    format!("{name}: {h}")
                },
            )),
            answer: None,
        },
        None => Reason {
            text: cap(&if ctx.is_empty() {
                format!("{name} отказал")
            } else {
                ctx
            }),
            answer: Some(cut(&format!("{name} {code}: {msg}"), MAX_ANSWER)),
        },
    }
}

/// Короткая причина одной строкой (для колонки «Причина» экрана): человеческий текст, а у неизвестной ошибки биржи —
/// «… (ответ биржи: …)». Принимает: исходный текст.
pub fn short(raw: &str) -> String {
    let r = human(raw);
    match r.answer {
        Some(a) => format!("{} (ответ биржи: {a})", r.text),
        None => r.text,
    }
}

/// Текст без служебных приставок и ссылок на номера решений — для событий без своего заголовка (dispatch.rs).
/// Принимает: текст. Возвращает: очищенный текст (переводы строк сохраняются).
pub fn tidy(raw: &str) -> String {
    clean(raw)
}

/// Ищет ошибку биржи с кодом. Возвращает: (позиция начала, id площадки, код, msg) или None.
fn find_code(s: &str) -> Option<(usize, &'static str, String, String)> {
    for (p, v) in CODE_PREFIXES {
        if let Some(pos) = s.find(p) {
            let rest = &s[pos + p.len()..];
            let (code, msg) = rest.split_once(':').unwrap_or((rest, ""));
            return Some((pos, v, code.trim().to_string(), msg.trim().to_string()));
        }
    }
    let pos = s.find(GATE_PREFIX)?;
    let rest = &s[pos + GATE_PREFIX.len()..];
    let (_, after) = rest.split_once(", ")?;
    let (label, msg) = after.split_once(':').unwrap_or((after, ""));
    Some((
        pos,
        "gate",
        label.trim().to_string(),
        msg.trim().to_string(),
    ))
}

/// Убирает внутренние слова шагов в начале и ссылки на номера решений «(решение 53)». Принимает: текст.
fn clean(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    for w in PREFIXES {
        if let Some(r) = s.strip_prefix(w) {
            s = r.to_string();
        }
    }
    // «(решение 53)», «(решения 40/45)», «, решение 118» — для журнала, человеку не нужны.
    while let Some(i) = s.find("(решени") {
        match s[i..].find(')') {
            Some(j) => s.replace_range(i..i + j + 1, ""),
            None => break,
        }
    }
    s.replace("  ", " ").replace(" .", ".").trim().to_string()
}

/// Соединяет контекст и причину. Принимает: контекст (может быть пустым), причину.
fn join(ctx: &str, why: &str) -> String {
    if ctx.is_empty() {
        why.to_string()
    } else {
        format!("{ctx} — {why}")
    }
}

/// Первая буква — заглавная.
fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Обрезает до n символов с «…». Принимает: текст, предел.
pub fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
    t = t.trim_end().to_string();
    t.push('…');
    t
}
