//! Файл: tests/outcome_more_231.rs
//! Что проверяет: дополнение решения 231 (Тигран, 08.11), чистая логика без сети:
//! 1) отказ остановленной очереди кошелька — код QueueHalted («не отправлено»), а не OutcomeUnknown: повтор по правилам
//!    шлюза разрешён (gateway/rules.rs retry_allowed), в «ошибки подряд» не считается (отказ до отправки,
//!    core/risk_count.rs), продажа — повтор безопасен (core/op_error.rs retry_safe), перевод — точно не ушёл
//!    (withdraw_unsent); есть в списке кодов формы правил (api/rules_codes.rs);
//! 2) Bybit, позднее выяснение вывода (venues/bybit/resolve.rs pick, transfers.rs same_rows): совпадение «сеть + адрес +
//!    сумма» (монета и время отобраны запросом); ровно одна запись — создан, ноль — пока не найден, больше одной —
//!    NotApplicable (человек). Строки — по полям ответа «Get Withdrawal Records» (chain, toAddress, amount, withdrawId);
//!    значения — условные (записанных ответов истории выводов Bybit нет, docs/TASKS.md).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{op_error, risk_count}, arb::gateway::{errors, rules}, arb::api::rules_codes,
//! arb::venues::bybit::{resolve, transfers}.

use rust_decimal::Decimal;
use serde_json::json;

use arb::core::op_error::OpError;
use arb::core::types::{Asset, VenueId, WithdrawRequest};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::bybit::{resolve, transfers};

#[test]
fn queue_halted_is_not_sent() {
    let e = GatewayError::new(
        ErrorCode::QueueHalted,
        None,
        "Не отправлено — очередь кошелька остановлена до ручной проверки: x",
    );
    assert!(arb::gateway::rules::retry_allowed(ErrorCode::QueueHalted));
    assert!(!arb::core::risk_count::counts(&e));
    let op = OpError::from(&e);
    assert!(!op.outcome_unknown());
    assert!(op.retry_safe());
    assert!(op.withdraw_unsent());
    let codes = arb::api::rules_codes::codes();
    let c = codes
        .iter()
        .find(|c| c.code == ErrorCode::QueueHalted)
        .unwrap();
    assert!(!c.no_retry);
    // «Чем найти» к нему не прикладывается — исход известен (не отправлено).
    let p = arb::gateway::pending::PendingRef::withdraw("w1");
    assert!(e.pending(p).pending.is_none());
}

/// Запрос вывода USDT по BSC (условные адрес и сумма).
fn req() -> WithdrawRequest {
    WithdrawRequest {
        venue: VenueId("bybit".into()),
        asset: Asset {
            ticker: "USDT".into(),
            bsc_contract: None,
        },
        network: "BSC".into(),
        address: "0xabc".into(),
        amount: Decimal::new(125, 1),
        deal_id: "20261108120000000CAKE".into(),
    }
}

#[test]
fn bybit_same_rows_by_chain_address_amount() {
    let rows = vec![
        json!({"withdrawId": "1", "chain": "BSC", "toAddress": "0xabc", "amount": "12.5"}),
        json!({"withdrawId": "2", "chain": "ETH", "toAddress": "0xabc", "amount": "12.5"}),
        json!({"withdrawId": "3", "chain": "BSC", "toAddress": "0xdef", "amount": "12.5"}),
        json!({"withdrawId": "4", "chain": "BSC", "toAddress": "0xabc", "amount": "12.4"}),
        json!({"withdrawId": "5", "chain": "BSC", "toAddress": "0xabc", "amount": "12.50"}),
    ];
    assert_eq!(transfers::same_rows(&rows, &req()), vec!["1", "5"]);
    assert_eq!(transfers::same_rows(&rows[..4], &req()), vec!["1"]);
    assert!(transfers::same_rows(&rows[1..4], &req()).is_empty());
}

#[test]
fn bybit_pick_exactly_one() {
    assert_eq!(resolve::pick("w1", &[]).unwrap(), None);
    assert_eq!(
        resolve::pick("w1", &["77".to_string()]).unwrap(),
        Some("77".to_string())
    );
    let many = resolve::pick("w1", &["77".to_string(), "78".to_string()]).unwrap_err();
    assert_eq!(many.code, ErrorCode::NotApplicable);
    assert!(many.message.contains("2 записей"));
}
