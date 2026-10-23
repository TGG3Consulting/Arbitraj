//! Файл: notify/tg_text.rs
//! Что делает: подгоняет текст сообщения под предел длины Telegram Bot API.
//! Кто вызывает: notify/telegram.rs (sendMessage), notify/tg_buttons.rs (sendMessage с кнопками, editMessageText).
//! От чего зависит: ничего.
//! Настройки: нет — предел задан протоколом.

/// Предел длины текста sendMessage — константа протокола:
/// «text … 1-4096 characters after entities parsing», https://core.telegram.org/bots/api#sendmessage
/// Тот же предел у editMessageText: «text … 1-4096 characters after entity parsing»,
/// https://core.telegram.org/bots/api#editmessagetext
const MAX_TEXT_CHARS: usize = 4096;

/// Обрезает текст до предела протокола MAX_TEXT_CHARS (sendMessage, editMessageText) с пометкой «…».
/// Считаем строже — в единицах UTF-16 (эмодзи = 2): так предел не будет превышен
/// при любом способе подсчёта на стороне Telegram. Принимает: текст. Возвращает: текст в пределе.
pub fn fit_text(text: &str) -> String {
    if text.encode_utf16().count() <= MAX_TEXT_CHARS {
        return text.to_string();
    }
    let mut t = String::new();
    let mut units = 0;
    for ch in text.chars() {
        units += ch.len_utf16();
        if units > MAX_TEXT_CHARS - 1 {
            break;
        }
        t.push(ch);
    }
    t.push('…');
    t
}
