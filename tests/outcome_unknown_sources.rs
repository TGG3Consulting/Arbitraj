//! Файл: tests/outcome_unknown_sources.rs
//! Что делает: откуда берётся «результат неизвестен» (OutcomeUnknown, решение 64) и что его не повторить.
//! - BingX: ордер или вывод, на который ответ не получен целиком (нет заголовков, тело замолчало или оборвалось, общий
//!   срок, HTTP 504 «шлюз BingX не дождался ответа», непонятный ответ) — OutcomeUnknown; соединение не установлено —
//!   Unreachable (запрос не ушёл); справочники и проверки (Op::Other) — как было (Timeout можно повторять).
//! - Повтор для OutcomeUnknown запрещён везде: gateway/rules.rs retry_allowed, в справочнике формы правил (no_retry —
//!   «Повторить» не предлагается). Ядро не повторяет при любых правилах — tests/core_outcome_unknown.rs.
//!
//! Тексты сбоев — из журнала живого запуска 2026-10-25 («error sending request», «error decoding response body»).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::{http, rules, errors}, arb::venues::bingx::errors, arb::api::rules_codes.

use std::time::Duration;

use arb::gateway::errors::ErrorCode;
use arb::gateway::http::{Fail, Limits};
use arb::gateway::rules::retry_allowed;
use arb::venues::bingx::errors::{Op, fail_for, from_fail, from_http, unsure};

fn lim() -> Limits {
    let ms = Duration::from_millis;
    Limits {
        head: ms(10_000),
        idle: ms(3_000),
        total: ms(60_000),
        max_bytes: 4_000_000,
    }
}

#[test]
fn order_or_withdraw_without_result_is_outcome_unknown() {
    let sent_no_result = [
        Fail::NoHead,
        Fail::Send("error sending request".into()),
        Fail::Stalled { got: 0 },
        Fail::Broken {
            got: 512,
            why: "error decoding response body".into(),
        },
        Fail::Total { got: 100 },
        Fail::NotText,
        Fail::TooBig { got: 4_000_001 },
    ];
    for f in &sent_no_result {
        for op in [Op::Trade, Op::Withdraw] {
            let e = unsure(from_fail(f, &lim()), op);
            assert_eq!(e.code, ErrorCode::OutcomeUnknown, "{op:?} {f:?}");
            assert!(e.message.contains("итог не получен"), "{}", e.message);
            // Путь rest.rs: код задаётся видом сбоя (всё, кроме Connect), а не кодом from_fail.
            assert_eq!(
                fail_for(f, &lim(), op).code,
                ErrorCode::OutcomeUnknown,
                "{op:?} {f:?}"
            );
        }
        // Справочники и проверки — без изменений (их можно повторять по правилу).
        assert_ne!(
            unsure(from_fail(f, &lim()), Op::Other).code,
            ErrorCode::OutcomeUnknown
        );
    }
    // Соединение не установлено — запрос не ушёл: не «неизвестно».
    let not_sent = Fail::Connect("error sending request".into());
    assert_eq!(
        unsure(from_fail(&not_sent, &lim()), Op::Trade).code,
        ErrorCode::Unreachable
    );
    assert_eq!(
        fail_for(&not_sent, &lim(), Op::Withdraw).code,
        ErrorCode::Unreachable
    );
    // HTTP 504 на ордер — «шлюз BingX не дождался ответа, результат нужно проверить».
    assert_eq!(
        unsure(from_http(504, ""), Op::Trade).code,
        ErrorCode::OutcomeUnknown
    );
    // Отказ BingX с понятной причиной — не «неизвестно».
    assert_eq!(
        unsure(from_http(401, ""), Op::Trade).code,
        ErrorCode::InvalidCredentials
    );
}

#[test]
fn retry_is_forbidden_only_for_outcome_unknown() {
    let codes = arb::api::rules_codes::codes();
    for c in &codes {
        assert_eq!(
            c.no_retry,
            c.code == ErrorCode::OutcomeUnknown,
            "{:?}",
            c.code
        );
        assert_eq!(retry_allowed(c.code), !c.no_retry);
    }
    assert!(codes.iter().any(|c| c.no_retry));
}
