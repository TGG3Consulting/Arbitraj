//! Файл: core/ids.rs
//! Что делает: идентификаторы ядра в формате UUID версии 4 (RFC 9562, раздел 5.4): 36 знаков — влезают
//! в callback_data кнопки Telegram (≤ 64 байт вместе с кодом кнопки, Issue #10).
//! Кто вызывает: core/manual.rs (id алерта и монет человека).
//! От чего зависит: getrandom (системный генератор случайных чисел).
//! Настройки: нет.

/// Новый UUID v4 строкой «xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx».
/// Ошибки: системный генератор недоступен — текст причины (алерт тогда не создаётся).
pub fn uuid_v4() -> Result<String, String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| format!("генератор случайных чисел: {e}"))?;
    // RFC 9562: версия 4 — старшие 4 бита байта 6; вариант 10 — старшие 2 бита байта 8.
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}

#[cfg(test)]
mod tests {
    /// Формат UUID v4: 36 знаков, версия 4, вариант 10, два вызова — разные id.
    #[test]
    fn uuid_v4_format() {
        let a = super::uuid_v4().unwrap();
        let b = super::uuid_v4().unwrap();
        assert_eq!(a.len(), 36);
        assert_ne!(a, b);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
    }
}
