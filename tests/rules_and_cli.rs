//! Файл: tests/rules_and_cli.rs
//! Что делает: проверяет выбор правила шлюза (площадка сильнее общего) и разбор аргументов запуска.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::rules, arb::cli. Моков нет — чистые функции.

use arb::core::types::VenueId;
use arb::gateway::errors::ErrorCode;
use arb::gateway::rules::{Rule, RuleAction, RuleScope, pick_rule};

/// Правило для теста.
fn rule(id: i64, scope: RuleScope, action: RuleAction) -> Rule {
    Rule {
        id,
        event: ErrorCode::OrderNotFilled,
        condition: None,
        action,
        scope,
        enabled: true,
    }
}

#[test]
fn venue_rule_beats_global() {
    let rules = vec![
        rule(1, RuleScope::All, RuleAction::CancelNoRetry),
        rule(
            2,
            RuleScope::Venue(VenueId("bingx".into())),
            RuleAction::StopVenue,
        ),
    ];
    let v = VenueId("bingx".into());
    assert_eq!(
        pick_rule(&rules, ErrorCode::OrderNotFilled, Some(&v))
            .unwrap()
            .id,
        2
    );
    let other = VenueId("gate".into());
    assert_eq!(
        pick_rule(&rules, ErrorCode::OrderNotFilled, Some(&other))
            .unwrap()
            .id,
        1
    );
    assert!(pick_rule(&rules, ErrorCode::SwapReverted, Some(&v)).is_none());
}

#[test]
fn disabled_rule_is_skipped() {
    let mut r = rule(1, RuleScope::All, RuleAction::CancelNoRetry);
    r.enabled = false;
    assert!(pick_rule(&[r], ErrorCode::OrderNotFilled, None).is_none());
}

#[test]
fn cli_requires_both_args() {
    let a = |v: &[&str]| arb::cli::parse(v.iter().map(|s| s.to_string()));
    assert!(a(&[]).is_err());
    assert!(a(&["--data-dir", "d"]).is_err());
    assert!(a(&["--data-dir", "d", "--listen", "127.0.0.1:8080"]).is_ok());
}
