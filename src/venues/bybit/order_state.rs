//! Файл: venues/bybit/order_state.rs
//! Что делает: доводит ордер Bybit до окончательного состояния: ждёт событие приватного потока (private_cache.rs) или
//! опрашивает ордер REST, пока он не станет Filled / Cancelled / PartiallyFilledCanceled / Rejected; сбой запроса —
//! повтор до срока; не дождался за venue.order_final_wait_ms — всегда отменяет остаток (правило Тиграна «не исполнился
//! — отменяем») и читает итог ещё venue.order_final_wait_ms. Ордер с неизвестным итогом постановки ищется по своему
//! orderLinkId; не найден — отмена по нему же и снова поиск. Комиссия — fills.rs.
//! Кто вызывает: trade.rs.
//! От чего зависит: rest.rs, num.rs, ctx.rs, private_cache.rs, fills.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms.
//!
//! Официальная документация:
//! - «Get Open & Closed Orders» GET /v5/order/realtime https://bybit-exchange.github.io/docs/v5/order/open-order —
//!   category, orderId | orderLinkId («openOnly param will be ignored when query by orderId or orderLinkId»); «After a
//!   server release or restart, filled, cancelled, and rejected orders … should only be queried through order history».
//! - «Get Order History» GET /v5/order/history https://bybit-exchange.github.io/docs/v5/order/order-list.
//! - enum orderStatus — закрытые: Rejected, PartiallyFilledCanceled («Only spot has this order status»), Filled,
//!   Cancelled; поля cumExecQty, cumExecValue, avgPrice, qty, rejectReason.
//! - «Cancel Order» POST /v5/order/cancel https://bybit-exchange.github.io/docs/v5/order/cancel-order — category,
//!   symbol, orderId | orderLinkId.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Окончательные состояния ордера спота по документации (enum orderStatus, «closed status»).
const FINAL: [&str; 4] = ["Filled", "Cancelled", "PartiallyFilledCanceled", "Rejected"];

/// Сроки ожидания: venue.order_poll_ms и venue.order_final_wait_ms. Ошибка NotConfigured — не задана.
pub fn times(ctx: &Ctx) -> GatewayResult<(Duration, Duration)> {
    Ok((
        Duration::from_millis(ctx.int("venue.order_poll_ms")?),
        Duration::from_millis(ctx.int("venue.order_final_wait_ms")?),
    ))
}

/// Что делает: окончательное состояние ордера. Принимает: окружение, символ, orderId.
/// Возвращает: запись ордера с окончательным статусом. Ошибки: OutcomeUnknown — не получено и после отмены.
pub async fn settle(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<Value> {
    let (poll, wait) = times(ctx)?;
    let id = ("orderId", order_id);
    if let Ok(s) = poll_final(ctx, id, poll, wait).await {
        return Ok(s);
    }
    let cancel = cancel(ctx, symbol, id).await;
    match poll_final(ctx, id, poll, wait).await {
        Ok(s) => Ok(s),
        Err(why) => {
            let c = cancel
                .err()
                .map(|e| format!("отмена: {}; ", e.message))
                .unwrap_or_default();
            Err(ctx.tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "Ордер {order_id} отправлен, но окончательное состояние не получено: {c}{why}"
                ),
            )))
        }
    }
}

/// Что делает: orderId по своему orderLinkId, когда итог постановки неизвестен. Поиск; не найден — отмена по
/// orderLinkId и поиск с повторами до venue.order_final_wait_ms. Возвращает: orderId; None — не найден. Ошибок нет.
pub async fn recover(ctx: &Ctx, symbol: &str, link_id: &str) -> Option<String> {
    let id = ("orderLinkId", link_id);
    if let Ok(Some(s)) = query(ctx, id).await {
        return num::text_opt(&s, "orderId");
    }
    let (poll, wait) = times(ctx).ok()?;
    let _ = cancel(ctx, symbol, id).await;
    let started = Instant::now();
    loop {
        if let Ok(Some(s)) = query(ctx, id).await {
            return num::text_opt(&s, "orderId");
        }
        if started.elapsed() >= wait {
            return None;
        }
        tokio::time::sleep(poll).await;
    }
}

/// Что делает: ожидание окончательного состояния не дольше wait: событие потока или запрос REST раз в poll.
/// Ошибка — текст последней причины.
async fn poll_final(
    ctx: &Ctx,
    id: (&'static str, &str),
    poll: Duration,
    wait: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        if id.0 == "orderId"
            && let Some(s) = ctx.private.order(id.1).filter(is_final)
        {
            return Ok(s);
        }
        let why = match query(ctx, id).await {
            Ok(Some(s)) if is_final(&s) => return Ok(s),
            Ok(Some(s)) => format!(
                "статус {}",
                num::text_opt(&s, "orderStatus").unwrap_or_default()
            ),
            Ok(None) => "ордер не найден".to_string(),
            Err(e) => format!("запрос статуса: {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(why);
        }
        // Событие потока будит раньше срока опроса.
        ctx.private.wait(poll).await;
    }
}

/// Запись ордера по orderId или orderLinkId: realtime, пусто — history. None — ордера нет ни там, ни там.
async fn query(ctx: &Ctx, id: (&'static str, &str)) -> GatewayResult<Option<Value>> {
    for path in ["/v5/order/realtime", "/v5/order/history"] {
        let q = vec![("category", "spot".to_string()), (id.0, id.1.to_string())];
        let d = rest::get(ctx, path, q, Op::Other).await?;
        if let Some(o) = num::arr(&d, "list")?.first() {
            return Ok(Some(o.clone()));
        }
    }
    Ok(None)
}

/// Отмена по orderId или orderLinkId. Ошибки: коды Bybit, связь.
async fn cancel(ctx: &Ctx, symbol: &str, id: (&'static str, &str)) -> GatewayResult<Value> {
    let mut body = json!({ "category": "spot", "symbol": symbol });
    body[id.0] = json!(id.1);
    rest::post(ctx, "/v5/order/cancel", &body, Op::Trade).await
}

/// Состояние окончательное. Ошибок нет.
fn is_final(s: &Value) -> bool {
    num::text_opt(s, "orderStatus").is_some_and(|st| FINAL.contains(&st.as_str()))
}
