//! Файл: venues/bingx/fees_pause.rs
//! Что делает: решение 204 — пауза запросов ставок комиссии BingX после кода 100410 («The endpoint trigger frequency
//! limit rule is currently in the disabled period and will be unblocked after <мс эпохи>»). Живой журнал 06.11.2026:
//! 393 записи venue.fees — фоновое обновление продолжало спрашивать ставку каждой пары, пока эндпоинт заблокирован, и
//! каждая пара писала свою запись. Теперь: момент разблокировки берётся из текста ответа, до него эндпоинт не
//! вызывается; полученные раньше ставки остаются в памяти (fees.rs меняет ставку только при успехе).
//! Кто вызывает: fees.rs (refresh_loop).
//! От чего зависит: chrono.
//! Настройки: нет своих (без метки в ответе пауза — venue.fee_retry_s, читает fees.rs).
//!
//! Официально: «Query Trading Commission Rate» GET /openApi/spot/v1/user/commissionRate — 2 запроса/с (шапка
//! fees.rs); формат текста 100410 в документации не описан — разбор по живому ответу 06.11.2026 (журнал бота).

use chrono::{DateTime, TimeZone, Utc};

/// Момент разблокировки из текста ответа 100410 («… will be unblocked after 1793995854885»). Принимает: текст
/// ошибки. Возвращает: момент (UTC) или None — метки нет или она не число мс эпохи. Ошибок нет.
pub fn unblock_at(msg: &str) -> Option<DateTime<Utc>> {
    const KEY: &str = "unblocked after ";
    let rest = &msg[msg.find(KEY)? + KEY.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let ms: i64 = digits.parse().ok()?;
    Utc.timestamp_millis_opt(ms).single()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Текст живого ответа из журнала бота 06.11.2026.
    #[test]
    fn parses_live_100410() {
        let m = "BingX, код 100410: code:100410:The endpoint trigger frequency limit rule is currently in the \
                 disabled period and will be unblocked after 1793995854885";
        let at = unblock_at(m).expect("метка");
        assert_eq!(at.timestamp_millis(), 1_793_995_854_885);
        assert!(unblock_at("BingX, код 100410: limit").is_none());
        assert!(unblock_at("unblocked after soon").is_none());
    }
}
