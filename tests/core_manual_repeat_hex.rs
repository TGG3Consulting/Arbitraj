//! Файл: tests/core_manual_repeat_hex.rs
//! Что делает: решение 251, ответ (в) Тиграна 10.11 — «тот же отказ» продажи (core/manual_repeat.rs::same) считает
//! hex-последовательности (0x…, хеши, адреса, длинные hex-хвосты) числами, как цифры: два отказа, отличающиеся только
//! адресом, хешем или суммой, — один и тот же отказ; разные по смыслу — разные.
//! Тексты — по образцу живого отказа 05.11 (tests/core_decision_177.rs, MISMATCH_1) и форм отказов, которые пишет
//! бот (адрес пула, хеш транзакции). Сеть и база не нужны: проверяется чистая функция.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::manual_repeat.

use arb::core::manual_repeat::{same, shape};

const ADDR_1: &str = "0x7f6b3e1a59c2d4e8b0a1f3c5d7e9b2a4c6e8f0a2";
const ADDR_2: &str = "0x0ed7e52944161450477ee417de9cd3a859b14fd0";
const HASH_1: &str = "0x9c1f4e2b7a3d5c8e0f6a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3e4f5a6";
const HASH_2: &str = "0xab12cd34ef56ab78cd90ef12ab34cd56ef78ab90cd12ef34ab56cd78ef90ab12";

/// Отказ продажи по шаблону: пул, транзакция, сумма.
fn refusal(pool: &str, tx: &str, amount: &str) -> String {
    format!(
        "[свой сборщик: прямой] пул {pool}: транзакция {tx} откатилась: V2TooLittleReceived, получено {amount} < minOut"
    )
}

#[test]
fn only_address_hash_amount_differ_is_same_refusal() {
    let a = refusal(ADDR_1, HASH_1, "1234.5678");
    let b = refusal(ADDR_2, HASH_2, "98.1");
    assert!(same(&a, &b), "{} / {}", shape(&a), shape(&b));
    // Hex без «0x» (хвост ответа узла) и hex-сумма — тоже числа.
    let c = format!("{a}; data 08c379a0abcdef");
    let d = format!("{b}; data 4e487b71ff00");
    assert!(same(&c, &d), "{} / {}", shape(&c), shape(&d));
    assert!(same(
        "value 0xde0b6b3a7640000 wei",
        "value 0x1bc16d674ec80000 wei"
    ));
    // Адрес из одних букв a–f после «0x» — тоже hex.
    assert!(same("пул 0xabcdef", "пул 0x123456"));
}

#[test]
fn different_meaning_is_different_refusal() {
    let a = refusal(ADDR_1, HASH_1, "1234.5678");
    let other = format!(
        "[свой сборщик: прямой] пул {ADDR_1}: транзакция {HASH_1} не попала в блок за send.max_wait_blocks"
    );
    assert!(!same(&a, &other));
    // Обычные слова из букв a–f («bad», «added») — не числа: разные слова — разный отказ.
    assert!(!same("bad route", "added route"));
    // Слово с не-hex буквами не сводится целиком: «V2TooLittleReceived» ≠ «V3TooMuchRequested».
    assert!(!same("V2TooLittleReceived", "V3TooMuchRequested"));
}
