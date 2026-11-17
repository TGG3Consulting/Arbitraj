//! Файл: tests/poloniex_deposit_addr_t165.rs
//! Что делает: задача t165, решение 379 — адрес депозита Poloniex, которого нет в ответе Deposit Addresses, бот создаёт
//! сам (New Currency Address POST /wallets/address, https://api-docs.poloniex.com/spot/api/private/wallet). Проверки:
//! адрес из ответа создания берётся той же проверкой (deposit_addr_new::pick), что и из ответа GET; пустой адрес и ответ
//! без поля — «адреса нет»; выбор «создать / запрет прогрева» — gateway/addr_claim.rs (decide, on_missing).
//! Без теста на записи: живого ответа POST нет — разбор на примере из документации (tests/data/poloniex_doc_2026-11-17);
//! записать живой ответ на сервере при первом создании (docs/TASKS.md, t165).
//! Кто вызывает: cargo test. От чего зависит: arb::venues::poloniex::deposit_addr_new, arb::gateway::addr_claim.
//! Моков нет.

use serde_json::{Value, json};

use arb::gateway::addr_claim::{self, Missing};
use arb::venues::poloniex::deposit_addr_new::{ADDRESS, pick};

/// Пример ответа из документации (JSON целиком).
fn example() -> Value {
    let path = "tests/data/poloniex_doc_2026-11-17/new_currency_address_example.json";
    let t = std::fs::read_to_string(path).unwrap_or_else(|_| panic!("нет файла {path}"));
    serde_json::from_str(&t).expect("JSON")
}

#[test]
fn created_address_from_doc_example() {
    let v = example();
    assert_eq!(
        pick(&v, ADDRESS).as_deref(),
        Some("0xae8d0d7c520daebb1580dabaa91f3ccf8ae492f5")
    );
}

#[test]
fn missing_or_empty_address_is_none() {
    // Ответ GET без имени монеты сети — адреса нет (тогда — создание или запрет прогрева).
    assert_eq!(pick(&example(), "ESIM"), None);
    assert_eq!(pick(&json!({ "address": "" }), ADDRESS), None);
    assert_eq!(pick(&json!({ "address": "  " }), ADDRESS), None);
    assert_eq!(pick(&json!({ "address": null }), ADDRESS), None);
}

#[tokio::test]
async fn create_only_outside_warm_scope() {
    assert_eq!(
        addr_claim::on_missing(),
        Missing::Create,
        "предпроверка создаёт"
    );
    let warm = addr_claim::forbid(async { addr_claim::on_missing() }).await;
    assert_eq!(warm, Missing::Forbidden, "прогрев без «да» не создаёт");
}
