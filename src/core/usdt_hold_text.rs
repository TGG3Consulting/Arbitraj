//! Файл: core/usdt_hold_text.rs
//! Что делает: решение 405 (t191, требование Тиграна 18.11: тексты — чётко и конкретно) — тексты резерва USDT
//! возвратов (usdt_hold.rs): отказ покупки (limits.rs zero_text — что хотели купить, сколько на счёте, сколько и чьё в
//! резерве, что делать), хвост причины у ручного перевода USDT (usdt_transfer_plan.rs) и чужого перегона
//! (usdt_return_attempt.rs, usdt_relay_retry.rs). Суммы — как в Telegram (notify/human.rs usd).
//! Кто вызывает: usdt_hold.rs (suffix — реэкспорт), limits.rs (zero_text).
//! От чего зависит: usdt_hold.rs (Held), notify/human.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::notify::human as hu;

use super::usdt_hold::Held;

/// Что делает: хвост причины: что в резерве, за кем и сколько — та же сумма, что вычтена (М7: из того же Held).
/// Принимает: резерв, тикер котируемой монеты (не используется в суммах — они в USDT как у возврата). Возвращает: пусто —
/// резерва нет. Ошибок нет.
pub fn suffix(h: &Held, _quote: &str) -> String {
    if h.amount <= Decimal::ZERO {
        return String::new();
    }
    let who: Vec<String> = h
        .lines
        .iter()
        .map(|(d, w, a)| format!("сделка {d} ({w}) — {}", hu::usd(*a)))
        .collect();
    format!(
        " — ещё {} в резерве возвратов USDT: {}; эти USDT ждут возврата, бот их не трогает (решение 405)",
        hu::usd(h.amount),
        who.join("; ")
    )
}

/// Что делает: отказ покупки — свободного после резерва нет. Начало — none_free (окно ставит метку «баланс»).
/// Принимает: начало («на X свободно 0 USDT»), резерв (с прочитанным свободным и площадкой), бюджет покупки (None —
/// предпроверка, бюджета ещё нет). Ошибок нет.
pub fn zero(head: &str, h: &Held, budget: Option<Decimal>) -> String {
    if h.amount <= Decimal::ZERO {
        return head.to_string();
    }
    let want = budget
        .map(|b| format!(", хотели купить на {}", hu::usd(b)))
        .unwrap_or_default();
    let seen = match (h.free, &h.place) {
        (Some(f), Some(p)) => format!("; на {p} сейчас {}", hu::usd(f)),
        (Some(f), None) => format!("; на счёте сейчас {}", hu::usd(f)),
        _ => String::new(),
    };
    format!(
        "{head} для покупки{want}{seen}{}. Покупку не делаем. Что сделать: дождитесь этих возвратов или разберите \
         «не вернулись» на экране «Ручное управление» → «Резерв USDT возвратов»",
        suffix(h, "USDT")
    )
}
