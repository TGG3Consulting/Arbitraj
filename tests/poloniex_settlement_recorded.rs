//! Файл: tests/poloniex_settlement_recorded.rs
//! Что делает: проверки расчётного круга Poloniex на записанных данных: (1) сверка суммы вывода после потери ответа
//! (withdraw_recover.rs) — запрошенная сумма и amount истории из учёта сделок SNDK 24.11.2026; (2) закрытый биржей вывод
//! USDC по Base (сеть ETHBASE) — справочник сетей видит его закрытым, ядро не пускает Poloniex в круг вывода.
//! Кто вызывает: cargo test.
//! От чего зависит: venues/poloniex/withdraw_recover.rs, venues/poloniex/networks_parse.rs,
//! tests/data/poloniex_withdraw_alias_2026-11-23/sndk_2026-11-24.json, tests/data/base_settlement_public_sources.json.

use arb::core::types::VenueId;
use arb::venues::poloniex::{networks_parse, withdraw_recover::amount_matches};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::str::FromStr;

/// Чтение JSON из файла. Ошибок нет (падение теста — файла нет).
fn json_file(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Запрос с 9 знаками, история — с 8 (отброшен последний): та же запись; другая сумма — нет.
#[test]
fn withdrawal_amount_matches_recorded_history() {
    let d = json_file("tests/data/poloniex_withdraw_alias_2026-11-23/sndk_2026-11-24.json");
    let rows = d.as_array().unwrap();
    assert!(!rows.is_empty());
    for r in rows {
        let requested = Decimal::from_str(r["withdraw_requested"].as_str().unwrap()).unwrap();
        let recorded = Decimal::from_str(r["sent"]["amount"].as_str().unwrap()).unwrap();
        assert_ne!(requested, recorded, "в записи суммы различаются по знакам");
        assert!(amount_matches(recorded, requested));
        let other = requested + Decimal::new(1, recorded.scale());
        assert!(!amount_matches(recorded, other));
    }
}

/// Живая строка сети ETHBASE монеты USDC (GET /v2/currencies/USDC, 25.11.2026): вывод закрыт, ввод открыт, контракт
/// Circle разобран. Строка networkList — без изменений; родительский объект монеты — по URL ответа (coin = USDC).
#[test]
fn base_usdc_withdrawal_closed_is_seen() {
    let d = json_file("tests/data/base_settlement_public_sources.json");
    let usdc = &d["cex"]["Poloniex"]["USDC"];
    assert!(
        usdc["url"]
            .as_str()
            .unwrap()
            .ends_with("/v2/currencies/USDC")
    );
    let row = usdc["base_rows"][0].clone();
    let chain = row["blockchain"].as_str().unwrap().to_string();
    assert_eq!(chain, "ETHBASE");
    let coin = json!({ "coin": "USDC", "networkList": [row] });
    // Единственное имя своих сетей здесь — значение venue.base_network [poloniex] (ETHBASE).
    let p =
        networks_parse::parse(&coin, chain.as_str(), &VenueId("poloniex".into()), false).unwrap();
    let net = p.list.iter().find(|n| n.network == chain).unwrap();
    assert!(!net.withdraw_enabled);
    assert!(net.deposit_enabled);
    assert_eq!(
        net.asset.contract.map(|a| a.to_string().to_lowercase()),
        Some("0x833589fcd6edb6e08f4c7c32d4f71b54bda02913".to_string())
    );
}
