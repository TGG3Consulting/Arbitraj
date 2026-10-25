//! Файл: tests/http_fail_codes.rs
//! Что делает: коды ошибок шлюза для сбоев HTTP-обмена (gateway/http.rs, решение 63) у BingX и DEX и повтор BingX на
//! резервный адрес. Unreachable — только «соединение не установлено» (запрос точно не дошёл); дошёл, а ответа
//! целиком нет (нет заголовков, тело замолчало, оборвалось, общий срок) — Timeout: для ордера и транзакции это
//! «результат неизвестен», а не «не отправлено» (правило «Unreachable → Retry» не отправит ордер второй раз).
//! Резервный адрес BingX — только если запрос не дошёл, или срок без ответа у справочников (Op::Other).
//! Тексты сбоев reqwest — из журнала живого запуска 2026-10-25 («error sending request», «error decoding response body»).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::gateway::http, arb::venues::{bingx::{errors, rest}, bsc_dex::errors}.

use std::time::Duration;

use arb::gateway::errors::ErrorCode;
use arb::gateway::http::{Fail, Limits};
use arb::venues::bingx::errors::{Op, from_fail as bingx_fail};
use arb::venues::bingx::rest::may_fallback;
use arb::venues::bsc_dex::errors::from_fail as dex_fail;

fn lim() -> Limits {
    let ms = Duration::from_millis;
    Limits {
        head: ms(10_000),
        idle: ms(3_000),
        total: ms(60_000),
        max_bytes: 4_000_000,
    }
}

fn all() -> Vec<(Fail, ErrorCode)> {
    vec![
        (
            Fail::Connect("error sending request".into()),
            ErrorCode::Unreachable,
        ),
        (Fail::NoHead, ErrorCode::Timeout),
        (
            Fail::Send("error sending request".into()),
            ErrorCode::Timeout,
        ),
        (Fail::Stalled { got: 65_536 }, ErrorCode::Timeout),
        (
            Fail::Broken {
                got: 196_608,
                why: "error decoding response body".into(),
            },
            ErrorCode::Timeout,
        ),
        (Fail::Total { got: 900_000 }, ErrorCode::Timeout),
        (Fail::TooBig { got: 4_000_001 }, ErrorCode::BadResponse),
        (Fail::NotText, ErrorCode::BadResponse),
    ]
}

#[test]
fn only_connect_is_unreachable() {
    for (f, code) in all() {
        assert_eq!(bingx_fail(&f, &lim()).code, code, "BingX {f:?}");
        assert_eq!(
            dex_fail(&f, &lim(), "узел BSC (HTTP)").code,
            code,
            "DEX {f:?}"
        );
        let text = f.text(&lim());
        if !matches!(f, Fail::Connect(_)) {
            assert!(
                !text.contains("нет связи (соединение не установлено)"),
                "{text}"
            );
        }
    }
}

#[test]
fn fallback_only_when_not_sent_or_reference_timeout() {
    for (f, _) in all() {
        let not_sent = matches!(f, Fail::Connect(_));
        assert_eq!(may_fallback(&f, Op::Trade), not_sent, "ордер: {f:?}");
        assert_eq!(may_fallback(&f, Op::Withdraw), not_sent, "вывод: {f:?}");
        let timeout = !matches!(f, Fail::TooBig { .. } | Fail::NotText);
        assert_eq!(
            may_fallback(&f, Op::Other),
            not_sent || timeout,
            "справочник: {f:?}"
        );
    }
}
