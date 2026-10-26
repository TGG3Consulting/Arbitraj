//! Файл: tests/bingx_account_update.rs
//! Что делает: решение 76, сторона BingX, — только на официальных и записанных данных:
//! - кадр ACCOUNT_UPDATE из примера официальной документации (docs-v3, Spot → Websocket Account Data → «Subscription
//!   account balance push», dataUpdateDemo, m = INIT) разбирается как снимок: изменений нет;
//! - причины m из той же страницы: DEPOSIT — поступление, WITHDRAW — вывод, ORDER — своё, INIT — снимок, прочие —
//!   постороннее;
//! - ожидаемый приход вывода биржи = сумма − комиссия вывода (credit_expect.rs, как costs.rs): настоящий вывод BNB с
//!   BingX 0,01998 при комиссии 0,0005 дошёл ровно 0,01948 (value транзакции —
//!   tests/data/native_2026-10-25/arb_bingx_bnb_withdraw_2026-10-25.json) — подходит при нулевом допуске.
//!
//! Разбор DEPOSIT и формы кадра на живом потоке — после записи (DECISIONS 76, открытые вопросы (а)–(в)).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bingx::account_update, arb::core::credit_match.

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::credit_match::fits;
use arb::core::types::{BalanceKind, VenueId};
use arb::venues::bingx::account_update::{Frame, kind, parse};

/// Пример кадра из официальной документации (dataUpdateDemo, текст как на странице).
const DOCS_DEMO: &str = r#"{"e":"ACCOUNT_UPDATE","a":{"B":[{"a":"USDT","bc":"0.00","cw":"0.00000000999999","wb":"0.00000000999999","lk":"0"}],"m":"INIT"},"E":1760067938102}"#;

#[test]
fn docs_demo_is_snapshot() {
    let v = VenueId("bingx".into());
    match parse(DOCS_DEMO, &v) {
        Frame::Update {
            wrapped,
            reason,
            changes,
        } => {
            assert!(!wrapped);
            assert_eq!(reason, "INIT");
            assert!(changes.is_empty());
        }
        other => panic!("не разобран: {other:?}"),
    }
    // Ответ на подписку из той же страницы — не ACCOUNT_UPDATE.
    let ack = r#"{"id":"e745cd6d-d0f6-4a70-8d5a-043e4c741b40","code":0,"msg":"","dataType":"","data":null}"#;
    assert_eq!(parse(ack, &v), Frame::Other);
}

#[test]
fn reasons_from_docs() {
    assert_eq!(kind("INIT"), None);
    assert_eq!(kind("DEPOSIT"), Some((BalanceKind::Deposit, false)));
    assert_eq!(kind("WITHDRAW"), Some((BalanceKind::Withdraw, false)));
    assert_eq!(kind("ORDER"), Some((BalanceKind::Other, true)));
    for m in [
        "FUNDING_FEE",
        "WITHDRAW_REJECT",
        "ADJUSTMENT",
        "ADMIN_DEPOSIT",
        "ASSET_TRANSFER",
        "AUTO_EXCHANGE",
    ] {
        assert_eq!(kind(m), Some((BalanceKind::Other, false)), "{m}");
    }
}

#[test]
fn bingx_withdraw_arrives_amount_minus_fee() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/native_2026-10-25/arb_bingx_bnb_withdraw_2026-10-25.json"
    );
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let tx = &rec["results"][0]["response"]["result"];
    let wei =
        u128::from_str_radix(tx["value"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    let got = Decimal::from_i128_with_scale(wei as i128, 18);
    let expected: Decimal =
        "0.01998".parse::<Decimal>().unwrap() - "0.0005".parse::<Decimal>().unwrap();
    assert!(fits(expected, got, Decimal::ZERO), "{expected} vs {got}");
}
