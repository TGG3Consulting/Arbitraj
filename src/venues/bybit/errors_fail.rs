//! Файл: venues/bybit/errors_fail.rs
//! Что делает: перевод сбоев HTTP-обмена с Bybit (нет связи, нет ответа, тело не текст или больше предела) в единые
//! коды шлюза; у операции с деньгами — «итог неизвестен». Вынесено из errors.rs без изменения логики (решение 263:
//! errors.rs вырос больше 200 строк); снаружи — как прежде, errors::from_fail / errors::fail_for (pub use).
//! Кто вызывает: errors.rs (pub use) → rest.rs и файлы адаптера.
//! От чего зависит: errors.rs (Op, unknown, venue), gateway/errors.rs, gateway/http.rs (Fail, Limits).
//! Настройки: нет.

use crate::gateway::errors::{ErrorCode, GatewayError};

use super::errors::{Op, unknown, venue};

/// Сбой HTTP-обмена: нет связи (Unreachable), запрос мог дойти без ответа (Timeout), тело не текст или больше
/// предела (BadResponse). Ошибок нет.
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
    GatewayError::new(code, Some(venue()), format!("Bybit: {}", f.text(lim)))
}

/// Сбой обмена у операции с деньгами: любой сбой, кроме «соединение не установлено», — «результат неизвестен».
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
