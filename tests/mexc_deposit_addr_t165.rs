//! Файл: tests/mexc_deposit_addr_t165.rs
//! Что делает: задача t165, решение 379 — адрес депозита MEXC, которого нет в ответе Deposit Address, бот создаёт сам
//! (Generate deposit address (supporting network) POST /api/v3/capital/deposit/address,
//! https://www.mexc.com/api-docs/spot-v3/wallet-endpoints/generate-deposit-address-supporting-network). Проверки:
//! запись сети выбирается из ответа создания тем же pick, что и из ответа GET (по отображаемому имени или коду сети);
//! чужая сеть — None; запись с memo видна вызывающему (он отказывает «требует memo»).
//! Без теста на записи: живого ответа POST нет — разбор на примере из документации (tests/data/mexc_doc_2026-11-17);
//! записать живой ответ на сервере при первом создании (docs/TASKS.md, t165).
//! Кто вызывает: cargo test. От чего зависит: arb::venues::mexc::deposit_addr_new. Моков нет.

use serde_json::Value;

use arb::venues::mexc::deposit_addr_new::pick;

/// Пример ответа из документации (JSON целиком).
fn example() -> Value {
    let path = "tests/data/mexc_doc_2026-11-17/generate_deposit_address_example.json";
    let t = std::fs::read_to_string(path).unwrap_or_else(|_| panic!("нет файла {path}"));
    serde_json::from_str(&t).expect("JSON")
}

#[test]
fn pick_by_network_from_doc_example() {
    let v = example();
    let r = pick(&v, "TRC20", "TRC20").expect("запись TRC20");
    assert_eq!(r["coin"], "USDT");
    assert_eq!(r["address"], "TXobiKkdciupZrhdvZyTSSLjE8CmZAufS");
    // Код сети совпал, отображаемое имя — нет: запись та же.
    assert!(pick(&v, "Tron(TRC20)", "TRC20").is_some());
    assert!(pick(&v, "BEP20(BSC)", "BSC").is_none(), "сети нет — None");
}

#[test]
fn memo_record_is_visible_to_caller() {
    let v = example();
    let r = pick(&v, "EOS", "EOS").expect("запись EOS");
    assert_eq!(r["memo"], "MX10068");
}

#[test]
fn single_object_is_taken_only_for_same_network() {
    let v = example();
    let one = v[0].clone();
    assert!(pick(&one, "TRC20", "TRC20").is_some());
    assert!(pick(&one, "BEP20(BSC)", "BSC").is_none());
}
