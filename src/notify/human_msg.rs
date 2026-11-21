//! Файл: notify/human_msg.rs
//! Что делает: сборщик человеческого алерта `Msg` (решения 164, 251) по единому шаблону (разделы — notify/alert.rs):
//! заголовок со значком уровня, «Сделка: <id>», карточка сделки («Монета», «Путь», «Где сейчас» — notify/alert_card.rs),
//! строки фактов, блок «Цифры», «Причина» (notify/reason.rs), «Что делает бот», «Что нужно от вас», технический хвост
//! «Подробно» (ответ биржи — коротко, полностью в журнале). Плюс разбор текста ядра «<id сделки>: …» (split_deal,
//! from_core). Текст без разметки — для экрана; Telegram размечает его в HTML (notify/alert.rs::telegram).
//! Кто вызывает: ядро и notify через notify/human.rs (переэкспорт).
//! От чего зависит: notify/reason.rs, notify/heads.rs, notify/human.rs (имя площадки), notify/alert.rs (подписи),
//! notify/alert_card.rs (карточка).
//! Настройки: нет.

use super::alert::{self as t, BOT_LABELS, FIGURE_LABELS};
use super::alert_card::{COIN, DEAL, DealCard, NOW, PATH};
use super::human::venue;

/// Сообщение алерта по шаблону решения 251. Без повторов строк.
#[derive(Debug, Clone, Default)]
pub struct Msg {
    head: String,
    card: Option<DealCard>,
    lines: Vec<String>,
    figures: Vec<String>,
    reason: Option<String>,
    bot: Option<String>,
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

    /// Как event, но для события без заголовка в notify/heads.rs — ℹ️ и название из каталога. Принимает: код, название.
    pub fn titled(code: &str, title: &str) -> Self {
        Self {
            head: super::heads::head(code, title),
            ..Self::default()
        }
    }

    /// Карточка сделки: монета, путь, где сейчас (core/deal_text.rs::card). Принимает: карточку.
    pub fn card(mut self, c: DealCard) -> Self {
        self.card = Some(c);
        self
    }

    /// Строка факта (пустая — пропускается).
    pub fn line(mut self, s: impl Into<String>) -> Self {
        let s = s.into();
        if !s.trim().is_empty() && !self.lines.contains(&s) {
            self.lines.push(s);
        }
        self
    }

    /// Строка «Подпись: значение». Подпись из FIGURE_LABELS — в блок «Цифры»; «Что делает бот» — в свой раздел.
    pub fn kv(self, label: &str, value: impl AsRef<str>) -> Self {
        if BOT_LABELS.contains(&label) {
            return self.bot(value.as_ref());
        }
        if FIGURE_LABELS.contains(&label) {
            return self.num(label, value);
        }
        self.line(format!("{label}: {}", value.as_ref()))
    }

    /// Цифра в блок «Цифры»: «• подпись: значение».
    pub fn num(mut self, label: &str, value: impl AsRef<str>) -> Self {
        let s = format!("{}{label}: {}", t::BULLET, value.as_ref());
        if !self.figures.contains(&s) {
            self.figures.push(s);
        }
        self
    }

    /// Причина: исходный текст переводится в человеческий (notify/reason.rs); неизвестная ошибка биржи — в хвост
    /// «Подробно». Пустая — пропускается.
    pub fn reason(mut self, raw: &str) -> Self {
        let r = super::reason::human(raw);
        if !r.text.is_empty() {
            // Решение 251: причина — с заглавной (как предложение).
            let mut c = r.text.chars();
            self.reason = c
                .next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str());
        }
        self.answer = r.answer;
        self
    }

    /// Что делает бот — одна-две строки (пустая — пропускается).
    pub fn bot(mut self, s: &str) -> Self {
        if !s.trim().is_empty() {
            // Номера решений человеку не нужны (как у причины, notify/reason.rs).
            self.bot = Some(super::reason::tidy(s));
        }
        self
    }

    /// Что нужно от человека — одна строка.
    pub fn todo(mut self, s: impl Into<String>) -> Self {
        self.todo = Some(s.into());
        self
    }

    /// id сделки — строкой «Сделка: <id>» под заголовком (по ней Telegram находит карточку сделки).
    pub fn id(mut self, s: impl Into<String>) -> Self {
        let s = s.into();
        if !s.is_empty() {
            self.id = Some(s);
        }
        self
    }

    /// Текст сообщения по шаблону (без разметки). Возвращает: строки через перевод строки; разделы — через пустую.
    pub fn render(&self) -> String {
        let mut out = vec![self.head.clone()];
        if let Some(id) = &self.id {
            out.push(format!("{DEAL}: {id}"));
        }
        if let Some(c) = &self.card {
            for (l, v) in [(COIN, &c.coin), (PATH, &c.path), (NOW, &c.now)] {
                if !v.is_empty() {
                    out.push(format!("{l}: {v}"));
                }
            }
        }
        if !self.figures.is_empty() {
            out.push(String::new());
            out.push(format!("{}:", t::FIGURES));
            out.extend(self.figures.iter().cloned());
            if !self.lines.is_empty() {
                out.push(String::new());
            }
        }
        out.extend(self.lines.iter().cloned());
        let tail: Vec<String> = [
            (t::REASON, &self.reason),
            (t::BOT, &self.bot),
            (t::TODO, &self.todo),
        ]
        .into_iter()
        .filter_map(|(l, v)| v.as_ref().map(|v| format!("{l}: {v}")))
        .collect();
        if !tail.is_empty() {
            out.push(String::new());
            out.extend(tail);
        }
        if let Some(a) = &self.answer {
            out.push(String::new());
            out.push(t::DETAILS.to_string());
            out.push(format!("Ответ биржи: {a}"));
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

/// Сообщение события из текста ядра «<id>: что случилось»: заголовок события, площадка, причина, id сделки.
/// Принимает: код события, площадку (id или None), текст. Возвращает: готовый текст по шаблону.
pub fn from_core(code: &str, venue_id: Option<&str>, text: &str) -> String {
    let (id, rest) = split_deal(text);
    let mut m = Msg::event(code);
    if let Some(v) = venue_id {
        m = m.kv("Площадка", venue(v));
    }
    m.reason(rest).id(id.unwrap_or_default()).render()
}
