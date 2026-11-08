//! Файл: tests/rules_inactive.rs
//! Что делает: решение 230, часть 3 — действия правил шлюза, которые для кода не действуют (gateway/rules.rs
//! inactive_reason): для отказа вывода (WithdrawRejected, InvalidAddress, WithdrawLocked) — «Повторить», «Стоп
//! площадка», «Стоп всё» (повтора нет, паузы нет — только предупреждение, решения 220, 225); для «результат
//! неизвестен» — «Повторить» (решение 64). Справочник формы (api/rules_codes.rs) отдаёт это в codes[].inactive.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::rules, arb::api::rules_codes, arb::core::auto_pause. Моков нет — чистые функции.

use arb::gateway::errors::ErrorCode as C;
use arb::gateway::rules::{RuleAction, action_kind, inactive_reason, withdraw_refusal};

#[test]
fn withdraw_refusal_codes_have_no_retry_and_no_stop() {
    for code in [C::WithdrawRejected, C::InvalidAddress, C::WithdrawLocked] {
        assert!(withdraw_refusal(code));
        assert!(
            arb::core::auto_pause::no_pause(code),
            "тот же перечень, что у пауз"
        );
        for kind in ["Retry", "StopVenue", "StopAll"] {
            assert!(inactive_reason(code, kind).is_some(), "{code:?} {kind}");
        }
        assert!(
            inactive_reason(code, "StopVenue")
                .unwrap()
                .contains("только предупреждение")
        );
        for kind in ["CancelNoRetry", "NotifyOnly"] {
            assert_eq!(inactive_reason(code, kind), None, "{code:?} {kind}");
        }
    }
}

#[test]
fn other_codes_unchanged() {
    assert!(inactive_reason(C::OutcomeUnknown, "Retry").is_some());
    assert_eq!(inactive_reason(C::OutcomeUnknown, "StopVenue"), None);
    for kind in [
        "Retry",
        "StopVenue",
        "StopAll",
        "CancelNoRetry",
        "NotifyOnly",
    ] {
        assert_eq!(inactive_reason(C::OrderNotFilled, kind), None);
        assert_eq!(inactive_reason(C::Timeout, kind), None);
    }
    assert!(!withdraw_refusal(C::DepositNotCredited));
}

#[test]
fn action_kind_matches_json() {
    let r = RuleAction::Retry {
        attempts: 1,
        pause_ms: 1,
    };
    assert_eq!(action_kind(&r), "Retry");
    let j = serde_json::to_value(&r).unwrap();
    assert!(
        j.get("Retry").is_some(),
        "вид — ключ объекта в JSON правила"
    );
    for a in [
        RuleAction::CancelNoRetry,
        RuleAction::StopVenue,
        RuleAction::StopAll,
        RuleAction::NotifyOnly,
    ] {
        assert_eq!(serde_json::to_value(&a).unwrap(), action_kind(&a));
    }
}

#[test]
fn form_meta_lists_inactive_actions() {
    let codes = arb::api::rules_codes::codes();
    let get = |c: C| codes.iter().find(|m| m.code == c).unwrap();
    let w = get(C::WithdrawLocked);
    assert!(w.no_retry);
    assert_eq!(
        w.inactive.keys().copied().collect::<Vec<_>>(),
        vec!["Retry", "StopAll", "StopVenue"]
    );
    let o = get(C::OutcomeUnknown);
    assert!(o.no_retry);
    assert_eq!(
        o.inactive.keys().copied().collect::<Vec<_>>(),
        vec!["Retry"]
    );
    assert!(get(C::OrderNotFilled).inactive.is_empty());
}
