//! Файл: core/exec_book_fail.rs
//! Что делает: решение 238 (ревью, п. 4) — память неудач снимка стакана по REST на пути ордера (exec_book.rs). REST не
//! ответил — неудача запоминается по (площадка, тикер площадки, сторона) на окно: exec.rest_book_retry_ms площадки
//! (пауза между попытками REST этого же пути), не задана — venue.rate_limit_pause_ms (пауза адаптера после «слишком
//! часто»); ни одна не задана — не запоминается (частота REST не ограничена — сказано в тексте ошибки). В окне новый
//! запрос REST не идёт: сразу ошибка с причиной прошлой неудачи и её возрастом. Так застрявшая монета («Ожидание»,
//! «Порог убытка» — котировка на каждый тик монеты) не бьёт REST биржи без предела при постоянных отказах.
//! Кто вызывает: core/exec_book.rs (book — recent перед запросом REST, failed — remember).
//! От чего зависит: exec_book.rs (память Inner.exec_books, setting), op_error.rs, gateway/errors.rs.
//! Настройки: exec.rest_book_retry_ms, venue.rate_limit_pause_ms (по площадке, необязательны).

use std::time::Instant;

use crate::core::types::VenueId;
use crate::gateway::errors::GatewayError;

use super::engine::Core;
use super::exec_book::{Key, RETRY, setting};
use super::op_error::OpError;

/// Запасное окно неудачи REST, если RETRY не задана, — пауза адаптера после «слишком часто».
pub const PAUSE: &str = "venue.rate_limit_pause_ms";

/// Окно после неудачи REST: RETRY площадки, не задана — PAUSE. Принимает: ядро, площадку.
/// Возвращает: (мс, имя настройки); None — не задана ни одна. Ошибок нет.
pub fn window(core: &Core, v: &VenueId) -> Option<(u64, &'static str)> {
    setting(core, v, RETRY)
        .map(|ms| (ms, RETRY))
        .or_else(|| setting(core, v, PAUSE).map(|ms| (ms, PAUSE)))
}

pub(super) struct Failure {
    at: Instant,
    why: String,
    waiting_for_local: bool,
}

/// Запоминает неудачу REST по ключу (если окно задано). Принимает: ядро, ключ, причину.
/// Возвращает: хвост текста ошибки — когда следующий запрос или что частота не ограничена. Ошибок нет.
pub fn remember(core: &Core, key: Key, why: &str, waiting_for_local: bool) -> String {
    let Some((ms, name)) = window(core, &key.0) else {
        return format!(" — частота запросов REST не ограничена: не задана ни {RETRY}, ни {PAUSE}");
    };
    core.inner
        .exec_books
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .fails
        .insert(
            key,
            Failure {
                at: Instant::now(),
                why: why.to_string(),
                waiting_for_local,
            },
        );
    if waiting_for_local {
        return format!(
            " — HTTP не отправлялся; появятся необходимые данные — REST доступен сразу, до этого повтор через {ms} мс ({name})"
        );
    }
    format!(" — следующий запрос REST не раньше чем через {ms} мс ({name})")
}

/// Неудача REST по ключу моложе окна — ошибка без запроса (без кода: алерт, как у exec_book failed; журнала нет —
/// неудача уже записана Error). Старше окна или окна нет — None (неудача снимается).
/// Принимает: ядро, ключ, ошибку потока. Возвращает: ошибку или None. Ошибок сверх этого нет.
pub fn recent(core: &Core, key: &Key, stream: &GatewayError, local_ready: bool) -> Option<OpError> {
    let w = window(core, &key.0);
    let mut g = core
        .inner
        .exec_books
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let fail = g.fails.get(key)?;
    if fail.waiting_for_local && local_ready {
        g.fails.remove(key);
        return None;
    }
    let why = &fail.why;
    let age = fail.at.elapsed().as_millis() as u64;
    match w {
        Some((ms, name)) if age < ms => Some(OpError::from(format!(
            "{}: {}; снимок стакана по REST не запрашивался: исполнимая котировка {age} мс назад не собралась ({why}), окно {ms} мс ({name})",
            key.0.0, stream.message
        ))),
        _ => {
            g.fails.remove(key);
            None
        }
    }
}
