//! Файл: notify/human_msg.rs
//! Что делает: сборщик человеческого алерта `Msg` (решение 164): первая строка — значок уровня и что случилось,
//! дальше по строке на факт без повторов, «Причина:» (notify/reason.rs), «Что делать:», «Ответ биржи:» для неизвестной
//! ошибки, внизу — id сделки. Плюс разбор текста ядра «<id сделки>: …» (split_deal, from_core).
//! Кто вызывает: ядро и notify через notify/human.rs (переэкспорт).
//! От чего зависит: notify/reason.rs, notify/heads.rs, notify/human.rs (имя площадки).
//! Настройки: нет.

use super::human::venue;

/// Сообщение алерта: заголовок со значком, строки фактов, причина, что делать, id внизу. Без повторов.
#[derive(Debug, Clone, Default)]
pub struct Msg {
    head: String,
    lines: Vec<String>,
    reason: Option<String>,
    answer: Option<String>,
    todo: Option<String>,
    id: Option<String>,
}

impl Msg {
    /// Новое сообщение. Принимает: значок уровня (STOP, WARN…) и что случилось — по-русски.
    pub fn new(icon: &str, head: impl AsRef<str>) -> Self {
        Self {
            head: format!("{icon} {}", head.as_ref()),
            ..Self::default()
        }
    }

    /// Сообщение с заголовком события из notify/heads.rs (значок и что случилось). Принимает: код события.
    pub fn event(code: &str) -> Self {
        Self {
            head: super::heads::head(code, code),
            ..Self::default()
        }
    }

    /// Строка факта (пустая — пропускается).
    pub fn line(mut self, s: impl Into<String>) -> Self {
        let s = s.into();
        if !s.trim().is_empty() && !self.lines.contains(&s) {
            self.lines.push(s);
        }
        self
    }

    /// Строка «Подпись: значение».
    pub fn kv(self, label: &str, value: impl AsRef<str>) -> Self {
        self.line(format!("{label}: {}", value.as_ref()))
    }

    /// Причина: исходный текст переводится в человеческий (notify/reason.rs); неизвестная ошибка биржи — строкой
    /// «Ответ биржи: …» в конце. Пустая — пропускается.
    pub fn reason(mut self, raw: &str) -> Self {
        let r = super::reason::human(raw);
        if !r.text.is_empty() {
            self.reason = Some(r.text);
        }
        self.answer = r.answer;
        self
    }

    /// Что делать — одна строка.
    pub fn todo(mut self, s: impl Into<String>) -> Self {
        self.todo = Some(s.into());
        self
    }

    /// id сделки (или объекта) — внизу, для поиска.
    pub fn id(mut self, s: impl Into<String>) -> Self {
        let s = s.into();
        if !s.is_empty() {
            self.id = Some(s);
        }
        self
    }

    /// Текст сообщения. Возвращает: строки через перевод строки.
    pub fn render(&self) -> String {
        let mut out = vec![self.head.clone()];
        out.extend(self.lines.iter().cloned());
        if let Some(r) = &self.reason {
            out.push(format!("Причина: {r}"));
        }
        if let Some(t) = &self.todo {
            out.push(format!("Что делать: {t}"));
        }
        if let Some(a) = &self.answer {
            out.push(format!("Ответ биржи: {a}"));
        }
        if let Some(id) = &self.id {
            out.push(String::new());
            out.push(format!("id {id}"));
        }
        out.join("\n")
    }
}

/// Текст ядра вида «<id сделки>: что случилось» → (id, остальное). id сделки — без пробелов и начинается с цифры
/// (core/ledger.rs open: время + тикер). Нет такого начала — (None, текст). Принимает: текст.
pub fn split_deal(text: &str) -> (Option<&str>, &str) {
    match text.split_once(": ") {
        Some((id, rest))
            if !id.is_empty()
                && !id.contains(' ')
                && id.starts_with(|c: char| c.is_ascii_digit()) =>
        {
            (Some(id), rest)
        }
        _ => (None, text),
    }
}

/// Сообщение события из текста ядра «<id>: что случилось»: заголовок события, площадка, причина, id внизу.
/// Принимает: код события, площадку (id или None), текст. Возвращает: готовый текст для Telegram.
pub fn from_core(code: &str, venue_id: Option<&str>, text: &str) -> String {
    let (id, rest) = split_deal(text);
    let mut m = Msg::event(code);
    if let Some(v) = venue_id {
        m = m.kv("Площадка", venue(v));
    }
    m.reason(rest).id(id.unwrap_or_default()).render()
}
