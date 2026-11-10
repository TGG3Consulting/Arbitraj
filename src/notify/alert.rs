//! Файл: notify/alert.rs
//! Что делает: решение 251 (Тигран 10.11: согласовано) — единый шаблон уведомлений о сделках и проблемах. Текст
//! собирает notify/human_msg.rs (Msg) по разделам этого модуля:
//!   ⛔ Заголовок — что случилось, одной строкой, значок уровня
//!   Сделка: <id> / Монета: <тикер> (<адрес коротко>) / Путь: BingX → DEX → BingX / Где сейчас: …
//!   строки фактов
//!   Цифры: • Вложено … • План … • Сейчас по замеру …
//!   Причина: … / Что делает бот: … / Что нужно от вас: …
//!   Подробно (полностью — в журнале): короткий технический хвост (ответ биржи)
//! Здесь же — перевод такого текста в HTML Telegram (telegram): карточка сделки (notify/alert_card.rs), подгонка под
//! предел длины, экранирование, жирные подписи, id сделки — моноширинным (нажать — скопировать), хвост — в
//! сворачиваемой цитате. Экран интерфейса получает тот же текст без разметки (Msg::render).
//! Telegram Bot API, Formatting options → HTML style (https://core.telegram.org/bots/api#formatting-options):
//! теги <b>, <code>, <blockquote expandable>; символы <, >, & вне тегов заменяются на &lt; &gt; &amp;; parse_mode
//! «HTML» у sendMessage и editMessageText (https://core.telegram.org/bots/api#sendmessage,
//! https://core.telegram.org/bots/api#editmessagetext). Предел 4096 символов считается «after entities parsing» —
//! поэтому текст подгоняется ДО разметки.
//! Кто вызывает: notify/human_msg.rs (подписи разделов), notify/dispatch.rs, dispatch_role.rs, mirror*.rs (telegram).
//! От чего зависит: notify/alert_card.rs, notify/tg_text.rs.
//! Настройки: нет — это вид сообщения, не торговые параметры.

use super::alert_card::{self, COIN, DEAL, NOW, PATH};
use super::tg_text::fit_text;

/// Подпись блока цифр (строка «Цифры:», под ней строки «• подпись: значение»).
pub const FIGURES: &str = "Цифры";
/// Приставка строки цифры.
pub const BULLET: &str = "• ";
/// Подписи разделов после цифр.
pub const REASON: &str = "Причина";
pub const BOT: &str = "Что делает бот";
pub const TODO: &str = "Что нужно от вас";
/// Строка, с которой начинается технический хвост (до конца текста) — в Telegram он свёрнут.
pub const DETAILS: &str = "Подробно (полностью — в журнале):";

/// Подписи строк, которые идут в блок «Цифры» (Msg::kv с такой подписью — цифра, а не строка факта): суммы сделки и
/// итог «продать сейчас» (notify/human.rs::sale_now_alert).
pub const FIGURE_LABELS: &[&str] = &[
    "Вложено",
    "Размер ордера",
    "План",
    "Факт",
    "Заработали",
    "Итого минус",
    "Выручка",
    "Выручка бота",
    "В итоге дня",
    "Убыток при продаже сейчас",
    "Прибыль при продаже сейчас",
    "Итог при продаже сейчас",
];

/// Подписи, которые Msg::kv переносит в раздел «Что делает бот» (так их называли тексты ядра до решения 251).
pub const BOT_LABELS: &[&str] = &[BOT, "Что бот делает"];

/// Подписи, которые в Telegram выделяются жирным (строка «Подпись: значение»).
const BOLD_LABELS: &[&str] = &[COIN, PATH, NOW, REASON, BOT, TODO];

/// Текст алерта для Telegram в HTML: карточка сделки (если есть «Сделка: <id>» и ядро знает сделку), подгонка под
/// предел длины, разметка. Принимает: текст (Msg::render или любой другой). Возвращает: HTML. Ошибок нет.
pub fn telegram(plain: &str) -> String {
    let text = match alert_card::deal_id(plain).and_then(alert_card::lookup) {
        Some(card) => alert_card::enrich(plain, &card),
        None => plain.to_string(),
    };
    html(&fit_text(&text))
}

/// Разметка HTML текста шаблона: всё экранируется; первая строка (заголовок) — жирная; подписи разделов —
/// жирные; id сделки — <code>; строки после DETAILS — в <blockquote expandable>. Принимает: текст. Ошибок нет.
pub fn html(plain: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut tail: Vec<String> = Vec::new();
    let mut in_tail = false;
    for (n, l) in plain.lines().enumerate() {
        if in_tail {
            tail.push(escape(l));
            continue;
        }
        if l == DETAILS {
            in_tail = true;
            out.push(format!("<b>{}</b>", escape(l)));
            continue;
        }
        out.push(line(l, n == 0));
    }
    if !tail.is_empty() {
        out.push(format!(
            "<blockquote expandable>{}</blockquote>",
            tail.join("\n")
        ));
    }
    out.join("\n")
}

/// Одна строка в HTML. Принимает: строку текста, первая ли (заголовок). Ошибок нет.
fn line(l: &str, first: bool) -> String {
    if first || l == format!("{FIGURES}:") {
        return format!("<b>{}</b>", escape(l));
    }
    if let Some(id) = l.strip_prefix(&format!("{DEAL}: ")) {
        return format!("<b>{DEAL}:</b> <code>{}</code>", escape(id));
    }
    for label in BOLD_LABELS {
        if let Some(rest) = l.strip_prefix(&format!("{label}: ")) {
            return format!("<b>{}:</b> {}", escape(label), escape(rest));
        }
    }
    escape(l)
}

/// Экранирование для HTML Telegram: «&», «<», «>» → &amp; &lt; &gt; (поддержаны документацией). Ошибок нет.
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
