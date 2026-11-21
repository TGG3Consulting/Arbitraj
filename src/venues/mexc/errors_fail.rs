//! Файл: venues/mexc/errors_fail.rs
//! Что делает: перевод сбоев HTTP-обмена с MEXC (нет связи, нет ответа, тело не текст или больше предела) в единые
//! коды шлюза; по операции с деньгами — «итог неизвестен». Вынесено из errors.rs без изменения логики (решение 263:
//! errors.rs вырос больше 200 строк); снаружи — как прежде, errors::from_fail / errors::fail_for (pub use).
//! Кто вызывает: errors.rs (pub use) → rest.rs, futures_rest.rs.
//! От чего зависит: errors.rs (Op, unknown, venue), gateway/errors.rs, gateway/http.rs (Fail, Limits).
//! Настройки: нет.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::errors::{Op, unknown, venue};

/// Сбой HTTP-обмена: нет связи (Unreachable), запрос мог дойти, а ответа нет (Timeout), тело не текст или больше
/// предела (BadResponse). Принимает: сбой, сроки. Возвращает: GatewayError.
pub fn from_fail(
    f: &crate::gateway::http::Fail,
    lim: &crate::gateway::http::Limits,
) -> GatewayError {
    use crate::gateway::http::Fail;
    let code = match f {
        Fail::Connect(_) => ErrorCode::Unreachable,
        Fail::NotText | Fail::TooBig { .. } => ErrorCode::BadResponse,
        _ => ErrorCode::Timeout,
    };
    GatewayError::new(code, Some(venue()), format!("MEXC: {}", f.text(lim)))
}

/// Сбой обмена по операции с деньгами (Trade, Withdraw, Move): любой сбой, кроме «соединение не установлено», —
/// «результат неизвестен». Принимает: сбой, сроки, операцию. Возвращает: GatewayError. Ошибок нет.
pub fn fail_for(
    f: &crate::gateway::http::Fail,
    lim: &crate::gateway::http::Limits,
    op: Op,
) -> GatewayError {
    let e = from_fail(f, lim);
    if op == Op::Other || f.not_sent() {
        return e;
    }
    unknown(e, op)
}
