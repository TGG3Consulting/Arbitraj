//! Файл: tests/bsc_dex_bundle_notify.rs
//! Что делает: проверяет, что уведомления жизненного цикла мульти-роутерного бандла (exec/bundle_notify.rs, решение 84,
//! фаза 80.8c-iii) формируются корректно — текст КАЖДОГО события несёт нужные факты (роутеры/ноги/транзакции/первый
//! nonce; причину отката; каналы и maxBlockNumber; номер блока и хеши; истечение и свободную очередь), и что все коды
//! событий зарегистрированы в каталоге notify/catalog.rs (иначе настройка «включить» и отправка в Telegram не работают —
//! settings/value.rs отвергает область события без кода). Без сети: текст строят чистые функции.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::bundle_notify, arb::notify::catalog.

use alloy_primitives::{B256, b256};

use arb::notify::catalog;
use arb::venues::bsc_dex::exec::bundle_notify as bn;

/// Собран: текст несёт число роутеров, ног, транзакций и первый nonce.
#[test]
fn assembled_text_carries_shape() {
    let t = bn::assembled_text(3, 5, 8, 2026);
    assert!(t.contains('3'), "нет числа роутеров: {t}");
    assert!(t.contains('5'), "нет числа ног: {t}");
    assert!(t.contains('8'), "нет числа транзакций: {t}");
    assert!(t.contains("2026"), "нет первого nonce: {t}");
    assert!(t.contains("собран"), "нет слова «собран»: {t}");
}

/// Симуляция прошла: текст несёт число свопов и ведёт к отправке.
#[test]
fn simulated_text_carries_routers() {
    let t = bn::simulated_text(4);
    assert!(t.contains('4'), "нет числа свопов: {t}");
    assert!(t.contains("прошла"), "нет исхода симуляции: {t}");
}

/// Симуляция откатилась: текст несёт причину.
#[test]
fn sim_reverted_text_carries_reason() {
    let t = bn::sim_reverted_text("шаг 2 откатился: INSUFFICIENT_OUTPUT_AMOUNT");
    assert!(t.contains("INSUFFICIENT_OUTPUT_AMOUNT"), "нет причины: {t}");
    assert!(
        t.contains("не отправлен"),
        "не сказано, что не отправлен: {t}"
    );
}

/// Отправлен: текст несёт каналы и предел блока.
#[test]
fn sent_text_carries_channels_and_max_block() {
    let t = bn::sent_text(
        &["bloxroute".to_string(), "48club".to_string()],
        130_556_920,
    );
    assert!(t.contains("bloxroute"), "нет канала: {t}");
    assert!(t.contains("48club"), "нет второго канала: {t}");
    assert!(t.contains("130556920"), "нет maxBlockNumber: {t}");
}

/// Отправлен без явных каналов (ни один не ответил ясно) — прочерк, но предел блока есть.
#[test]
fn sent_text_empty_channels() {
    let t = bn::sent_text(&[], 100);
    assert!(t.contains("100"), "нет maxBlockNumber: {t}");
    assert!(t.contains('—'), "нет прочерка для пустых каналов: {t}");
}

/// Включён: текст несёт номер блока и хеши транзакций.
#[test]
fn included_text_carries_block_and_hashes() {
    let h1: B256 = b256!("1111111111111111111111111111111111111111111111111111111111111111");
    let h2: B256 = b256!("2222222222222222222222222222222222222222222222222222222222222222");
    let t = bn::included_text(&[h1, h2], Some(130_556_918));
    assert!(t.contains("130556918"), "нет номера блока: {t}");
    assert!(
        t.contains("0x1111111111111111111111111111111111111111111111111111111111111111"),
        "нет первого хеша: {t}"
    );
    assert!(
        t.contains("0x2222222222222222222222222222222222222222222222222222222222222222"),
        "нет второго хеша: {t}"
    );
    assert!(t.contains("включён"), "нет исхода: {t}");
}

/// Включён без прочитанного номера блока — хеши всё равно есть, текст валиден.
#[test]
fn included_text_block_unknown() {
    let h: B256 = b256!("3333333333333333333333333333333333333333333333333333333333333333");
    let t = bn::included_text(&[h], None);
    assert!(t.contains("включён"), "нет исхода: {t}");
    assert!(
        t.contains("0x3333333333333333333333333333333333333333333333333333333333333333"),
        "нет хеша: {t}"
    );
}

/// Не попал в блок: текст несёт предел блока, истечение (газ не потрачен) и свободную очередь.
#[test]
fn not_included_text_carries_expiry_and_free_queue() {
    let t = bn::not_included_text(130_556_925);
    assert!(t.contains("130556925"), "нет maxBlockNumber: {t}");
    assert!(t.contains("истёк"), "не сказано про истечение: {t}");
    assert!(t.contains("газ не потрачен"), "не сказано про газ: {t}");
    assert!(
        t.contains("свободна"),
        "не сказано про свободную очередь: {t}"
    );
}

/// Все коды событий бандла зарегистрированы в каталоге (иначе включение/Telegram не работают).
#[test]
fn all_bundle_codes_in_catalog() {
    for code in [
        bn::ASSEMBLED,
        bn::SIMULATED,
        bn::SIM_REVERTED,
        bn::SENT,
        bn::INCLUDED,
        bn::NOT_INCLUDED,
    ] {
        let def = catalog::find(code).unwrap_or_else(|| panic!("нет события в каталоге: {code}"));
        assert_eq!(def.group, "Бандл свопов", "не та группа для {code}");
        assert!(!def.title.is_empty(), "пустой заголовок для {code}");
    }
}
