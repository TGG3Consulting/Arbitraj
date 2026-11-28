//! Файл: tests/bingx_settle.rs
//! Что делает: расчётный круг USDC (сеть Base) у BingX на живых ответах BingX 2026-11-21 (tests/data/bingx_2026-11-21,
//! без изменений внутри объектов):
//! 1) символ расчётной пары в справочнике пар — «USDC-USDT» (status 1, покупка и продажа через API открыты), и
//!    settle::matches принимает заявку с этим символом только для пары (USDC, USDT), а не для перевёрнутой или чужой;
//! 2) в config/getall у USDC есть сеть «BASE» (имя для venue.base_network) с контрактом USDC Base
//!    0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913, вывод и ввод открыты — разбор адаптера wallet::parse_networks;
//! 3) без прочитанного из API адреса депозита (памяти walletType) зачисление на торговый счёт не подтверждено
//!    (deposit_wallet.rs, решение Тиграна 28.11.2026).
//!
//! Ответа «Query Order details», истории выводов и истории депозитов с сервера в tests/data нет — их разбор здесь не
//! проверяется (список нужных записей — src/venues/bingx/README.md, «Расчётный круг USDC Base»).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bingx::{settle, wallet}, arb::gateway::pending, arb::core::types. Моков нет.

use alloy_primitives::Address;
use arb::core::types::VenueId;
use arb::gateway::pending::PendingRef;
use arb::venues::bingx::{settle, wallet};
use rust_decimal::Decimal;
use serde_json::Value;

const DIR: &str = "tests/data/bingx_2026-11-21";

fn json(name: &str) -> Value {
    let raw = std::fs::read_to_string(format!("{DIR}/{name}")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// Объект пары из записанного справочника common/symbols.
fn pair(symbol: &str) -> Value {
    let s = json("symbols.json");
    let list = s["data"]["symbols"].as_array().expect("список пар");
    list.iter()
        .find(|p| p["symbol"] == symbol)
        .cloned()
        .unwrap_or_else(|| panic!("пары {symbol} нет в записи"))
}

#[test]
fn settlement_symbol_matches_only_its_pair() {
    let usdc = pair("USDC-USDT");
    assert_eq!(usdc["status"], 1);
    assert_eq!(usdc["apiStateBuy"], true);
    assert_eq!(usdc["apiStateSell"], true);
    let sym = usdc["symbol"].as_str().unwrap();
    let p = PendingRef::order(sym, "b123456789_deal", Decimal::from(5));
    assert!(settle::matches(&p, "USDC", "USDT"));
    assert!(!settle::matches(&p, "USDT", "USDC"), "перевёрнутая пара");
    assert!(!settle::matches(&p, "USDC", "USDC"));
    assert!(!settle::matches(&p, "", "USDT"));
    // Другая реальная пара той же записи — не расчётная.
    let other = json("symbols.json");
    let list = other["data"]["symbols"].as_array().unwrap();
    let foreign = list
        .iter()
        .filter_map(|p| p["symbol"].as_str())
        .find(|s| *s != sym)
        .unwrap();
    let q = PendingRef::order(foreign, "b123456789_deal", Decimal::from(5));
    assert!(!settle::matches(&q, "USDC", "USDT"));
    // Заявка вывода (без символа) расчётной парой не считается.
    assert!(!settle::matches(
        &PendingRef::withdraw("w1"),
        "USDC",
        "USDT"
    ));
}

#[test]
fn usdc_base_network_from_live_config() {
    let cfg = json("config_getall.json");
    let venue = VenueId("bingx".into());
    let nets = wallet::parse_networks(&cfg["data"], "BASE", &venue).unwrap();
    let base = nets
        .iter()
        .find(|n| n.asset.ticker == "USDC" && n.network == "BASE")
        .expect("USDC в сети BASE");
    let usdc_base: Address = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
        .parse()
        .unwrap();
    assert_eq!(base.asset.contract, Some(usdc_base));
    assert!(base.withdraw_enabled && base.deposit_enabled);
}

/// Решение Тиграна 28.11.2026: пока адрес депозита не прочитан из API (памяти walletType нет) или ключ API не задан —
/// зачисление на торговый счёт не подтверждено. Записанного ответа deposit/address с walletType в tests/data нет —
/// положительный случай здесь не проверяется (список записей — README адаптера).
#[test]
fn no_wallet_type_means_no_credit() {
    let types = arb::venues::bingx::deposit_wallet::WalletTypes::default();
    assert!(!types.trading_credit(Some(1), "BASE"));
    assert!(!types.trading_credit(None, "BASE"));
}
