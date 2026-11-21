//! Файл: tests/core_usdt_return_tolerance.rs
//! Что делает: решение 227 (допуск, Тигран 08.11 «по копейкам — если это меньше одного USDT, то ок») на числах живой
//! сделки 20261108104958308JCT: часть DEX 342.29 USDT, биржа/кошелёк показали на копейки меньше. Ядро — на настоящей
//! SQLite без площадок в реестре (моков нет), допуск usdt.return_tolerance_usd читается из хранилища настроек ядра
//! (usdt_return_attempt::amount_for — та же функция, что у повтора и «Перегнать»). Проверяется: свободно 342.00 при
//! части 342.29 и допуске 1 → уходит 342.00; свободно 340 → отказ «чужие USDT не трогаем»; допуск не задан → отказ;
//! свободно больше части → ровно часть (допуск не увеличивает); допуск площадки сильнее общего.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, tests/usdt_return_common.

mod usdt_return_common;

use std::str::FromStr;

use arb::core::types::VenueId;
use arb::core::usdt_return_attempt::{amount_for, tolerance};
use arb::settings::value::Scope;
use rust_decimal::Decimal;
use serde_json::json;
use usdt_return_common::{dir, start, start_with};

/// Число из строки. Ошибок нет (тестовые литералы).
fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Площадка части JCT (кошелёк w7 на DEX).
fn dex() -> VenueId {
    VenueId("bsc_dex".into())
}

#[tokio::test]
async fn jct_shortfall_within_tolerance_sends_free() {
    let tol = vec![("usdt.return_tolerance_usd", Scope::Global, json!("1"))];
    let (core, _j) = start_with(&dir("tol-on"), None, tol).await;
    assert_eq!(tolerance(&core, &dex()), Some(d("1")));
    // На 0.29 меньше части — в пределах допуска 1: уходит свободный остаток.
    assert_eq!(
        amount_for(&core, &dex(), d("342.00"), d("342.29")),
        Ok(d("342.00"))
    );
    // Нехватка 2.29 больше допуска — как без допуска: ничего не отправляется.
    let e = amount_for(&core, &dex(), d("340"), d("342.29")).unwrap_err();
    assert!(e.contains("чужие USDT не трогаем"), "{e}");
    // Свободно больше части — ровно часть: допуск сумму не увеличивает (387.56 — живой баланс w7).
    assert_eq!(
        amount_for(&core, &dex(), d("387.56"), d("342.29")),
        Ok(d("342.29"))
    );
    // Свободно 0 при крошечной части — не «отправить 0».
    assert!(amount_for(&core, &dex(), Decimal::ZERO, d("0.5")).is_err());
}

#[tokio::test]
async fn jct_no_tolerance_is_strict() {
    let (core, _j) = start(&dir("tol-off"), None).await;
    assert_eq!(tolerance(&core, &dex()), None);
    let e = amount_for(&core, &dex(), d("342.00"), d("342.29")).unwrap_err();
    assert!(e.contains("чужие USDT не трогаем"), "{e}");
}

#[tokio::test]
async fn venue_tolerance_overrides_global() {
    let tol = vec![
        ("usdt.return_tolerance_usd", Scope::Global, json!("1")),
        (
            "usdt.return_tolerance_usd",
            Scope::Venue("bsc_dex".into()),
            json!("0.1"),
        ),
    ];
    let (core, _j) = start_with(&dir("tol-venue"), None, tol).await;
    // У bsc_dex своё 0.1 — нехватка 0.29 больше: отказ; у mexc — общее 1: уходит остаток.
    assert!(amount_for(&core, &dex(), d("342.00"), d("342.29")).is_err());
    let mexc = VenueId("mexc".into());
    assert_eq!(
        amount_for(&core, &mexc, d("342.00"), d("342.29")),
        Ok(d("342.00"))
    );
}
