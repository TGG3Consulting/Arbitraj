//! Файл: tests/poloniex_deposit_addr_t165.rs
//! Что делает: задача t165, решение 379 — адрес депозита Poloniex, которого нет в ответе Deposit Addresses, бот создаёт
//! сам (New Currency Address POST /wallets/address, https://api-docs.poloniex.com/spot/api/private/wallet). Проверки:
//! адрес из ответа создания берётся той же проверкой (deposit_addr_new::pick), что и из ответа GET; пустой адрес и ответ
//! без поля — «адреса нет»; выбор «создать / запрет прогрева» — gateway/addr_claim.rs (decide, on_missing).
//! Без теста на записи: живого ответа POST нет — разбор на примере из документации (tests/data/poloniex_doc_2026-11-17);
//! записать живой ответ на сервере при первом создании (docs/TASKS.md, t165).
//! Ревью 2: однозначность имени монеты сети (networks_parse::coin_clash) — на чистой функции: записанного ответа
//! /v2/currencies в tests/data нет (tests/poloniex_real_data.rs ждёт запись cex_samples_2026_11_05); вход — по форме примера
//! документации (TRX: coin «TRXETH» в сети ETH, «TRX» в сети TRX).
//! Кто вызывает: cargo test. От чего зависит: arb::venues::poloniex::deposit_addr_new, arb::gateway::addr_claim.
//! Моков нет.

use serde_json::{Value, json};

use arb::gateway::addr_claim::{self, Missing};
use arb::venues::poloniex::deposit_addr_new::{ADDRESS, pick};
use arb::venues::poloniex::networks_parse::coin_clash;

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

/// Справочник монеты по форме /v2/currencies (networkList: blockchain, coin). Ошибок нет.
fn currency(nets: &[(&str, &str)]) -> Value {
    let list: Vec<Value> = nets
        .iter()
        .map(|(b, c)| json!({ "blockchain": b, "coin": c }))
        .collect();
    json!([{ "coin": "TRX", "networkList": list }])
}

#[test]
fn coin_name_of_network_must_be_unique() {
    // Имена разные (как в примере документации) — однозначно.
    let ok = currency(&[("ETH", "TRXETH"), ("TRX", "TRX")]);
    assert_eq!(coin_clash(&ok, "TRX", "TRX"), None);
    assert_eq!(coin_clash(&ok, "TRX", "ETH"), None);
    // То же имя у другой сети — неоднозначно, называется эта сеть.
    let bad = currency(&[("ETH", "TRX"), ("BSC", "trx")]);
    assert_eq!(coin_clash(&bad, "TRX", "BSC").as_deref(), Some("ETH"));
    // Сети нет — None (отказ даёт chain_coin раньше).
    assert_eq!(coin_clash(&ok, "TRX", "BSC"), None);
}

/// Ревью t165 (заход 3, Н-3.1): классификация неудачи создания — чистая логика на функциях ошибок адаптеров.
/// Явный отказ (4xx с кодом биржи) и 429 — Refused (повтор после срока); 2xx без разбираемого тела и сбой сервера
/// (5xx) — «итог неизвестен» → Sent (повторного создания нет до перезапуска).
#[test]
fn create_failure_kind_poloniex_and_kraken() {
    use arb::gateway::addr_create::{Failure, post_failure};
    use arb::venues::{kraken, poloniex};

    let pe = poloniex::errors::Op::Create;
    let kind = |e: arb::gateway::errors::GatewayError| post_failure(&e.code);
    // Poloniex.
    let rejected = poloniex::errors::from_code(400, "21000", "отказ", pe, "");
    assert_eq!(
        kind(poloniex::errors::unsure(rejected, pe)),
        Failure::Refused
    );
    let limit = poloniex::errors::from_code(429, "429", "часто", pe, "");
    assert_eq!(kind(poloniex::errors::unsure(limit, pe)), Failure::Refused);
    let unparsed = poloniex::errors::ok_unparsed(poloniex::errors::bad("тело не JSON", ""), pe);
    assert_eq!(kind(poloniex::errors::unsure(unparsed, pe)), Failure::Sent);
    let server = poloniex::errors::from_code(500, "500", "сбой", pe, "");
    assert_eq!(kind(poloniex::errors::unsure(server, pe)), Failure::Sent);
    // Прочие операции разбор 2xx не меняет.
    let other = poloniex::errors::Op::Other;
    let plain = poloniex::errors::ok_unparsed(poloniex::errors::bad("x", ""), other);
    assert_eq!(plain.code, arb::gateway::errors::ErrorCode::BadResponse);

    // Kraken (claim).
    let ke = kraken::errors::Op::Create;
    let rejected = kraken::errors::from_funding(400, "{}", ke);
    assert_eq!(kind(kraken::errors::unsure(rejected, ke)), Failure::Refused);
    let limit = kraken::errors::from_funding(429, "{}", ke);
    assert_eq!(kind(kraken::errors::unsure(limit, ke)), Failure::Refused);
    let unparsed = kraken::errors::ok_unparsed(kraken::errors::bad("тело не JSON", ""), ke);
    assert_eq!(kind(kraken::errors::unsure(unparsed, ke)), Failure::Sent);
    let server = kraken::errors::from_funding(503, "{}", ke);
    assert_eq!(kind(kraken::errors::unsure(server, ke)), Failure::Sent);
}
