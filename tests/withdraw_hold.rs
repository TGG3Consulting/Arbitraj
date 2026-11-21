//! Файл: tests/withdraw_hold.rs
//! Что делает: решение 150 (удержания вывода, Тигран 04.11.2026). Записанного ответа MEXC deposit/hisrec со статусом
//! 9 PRE_SUCCESS пока нет, поэтому:
//! - расчёт «можно вывести» (core::precheck_withdraw::verdict) — на числах живого случая 04.11 (сделка
//!   20261104173851521BABYDOGE: «предварительно зачислено 45 USDT, 12 из 20 подтверждений», отказ вывода MEXC 16021);
//! - разбор ответа MEXC (venues::mexc::deposit_hold::parse) — #[ignore] до приватной записи на живом боте:
//!   ответ GET /api/v3/capital/deposit/hisrec (без параметров) в момент, когда есть депозит со status = 9, — в
//!   tests/data/mexc_deposit_hisrec_pre_success.json (без ключей; адреса можно оставить), затем
//!   `cargo test --test withdraw_hold -- --ignored` и снять #[ignore].
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::precheck_withdraw, arb::venues::mexc::deposit_hold. Моков нет.

use rust_decimal::Decimal;

use arb::core::precheck_withdraw::verdict;

/// Число из строки (тестовые суммы живого случая).
fn d(s: &str) -> Decimal {
    s.parse().unwrap()
}

/// Текст удержания живого случая.
const WHY: &str =
    "предварительно зачислено 45 USDT (подтверждений 12, нужно для вывода 20, сеть BSC)";

#[test]
fn hold_45_usdt_blocks_purchase_over_withdrawable() {
    // Свободно 50 USDT, из них 45 предварительно зачислены: вывести можно 5 — покупка на 10 не пройдёт.
    let e = verdict("mexc", "USDT", d("50"), d("45"), d("10"), WHY).unwrap_err();
    assert!(
        e.starts_with("mexc: предварительно зачислено 45 USDT"),
        "{e}"
    );
    assert!(e.contains("вывод не пройдёт"), "{e}");
    assert!(
        e.contains("можно вывести 50 − 45 = 5 USDT меньше покупки 10 USDT"),
        "{e}"
    );
}

#[test]
fn hold_45_usdt_allows_purchase_within_withdrawable() {
    // Свободно 100, удержано 45: вывести можно 55 — покупка на 55 проходит (граница), на 55.0001 — нет.
    assert!(verdict("mexc", "USDT", d("100"), d("45"), d("55"), WHY).is_ok());
    assert!(verdict("mexc", "USDT", d("100"), d("45"), d("55.0001"), WHY).is_err());
}

#[test]
fn no_hold_changes_nothing() {
    // Удержаний нет — проверка ничего не меняет, даже если покупка больше свободного (это решает buy_cap/refit).
    assert!(verdict("mexc", "USDT", d("5"), Decimal::ZERO, d("10"), "").is_ok());
}

#[test]
#[ignore = "нет приватной записи deposit/hisrec со status = 9 — шапка файла"]
fn recorded_hisrec_pre_success_parse() {
    let t = std::fs::read_to_string("tests/data/mexc_deposit_hisrec_pre_success.json")
        .expect("запись deposit/hisrec");
    let v: serde_json::Value = serde_json::from_str(&t).unwrap();
    let hold = arb::venues::mexc::deposit_hold::parse(&v).unwrap();
    assert!(
        !hold.is_empty(),
        "в записи должен быть депозит со status = 9"
    );
    for h in &hold.items {
        assert!(h.amount > Decimal::ZERO && !h.asset.is_empty(), "{h:?}");
    }
}
