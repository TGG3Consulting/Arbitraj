//! Файл: venues/coinex/order_state.rs
//! Что делает: доводит ордер CoinEx до окончательного итога: опрос order-status по order_id до filled / part_canceled /
//! canceled (сбой запроса — повтор до срока); не дождался за venue.order_final_wait_ms — отмена остатка (правило
//! Тиграна) и опрос ещё venue.order_final_wait_ms; ордер так и не найден — итог по сделкам ордера (deals.rs): ордер IOC
//! без исполнения CoinEx не сохраняет, и «нет сделок» при нулевом исполнении в ответе постановки — факт «не исполнился».
//! Итог в формате ядра — order_result.rs.
//! Кто вызывает: trade.rs, order_rules.rs (times — проверка до постановки).
//! От чего зависит: rest.rs, num.rs, order_result.rs, deals.rs, order_rules.rs (Prepared), ctx.rs.
//! Настройки: venue.order_poll_ms, venue.order_final_wait_ms, venue.my_trades_limit [coinex].
//!
//! Официально:
//! - Get Order Status GET /spot/order-status?market=&order_id= https://docs.coinex.com/api/v2/spot/order/http/get-order-status
//!   — status (order_status: open, part_filled, filled, part_canceled, canceled), amount, filled_amount, filled_value,
//!   base_fee, quote_fee, discount_fee; «After an order without any transactions is canceled, the server will not save the
//!   order» — кода ответа «ордер не найден» в документации нет (Error Handling), поэтому любой отказ запроса здесь —
//!   «не нашли», а факт берётся из сделок.
//! - Cancel Order POST /spot/cancel-order {market, market_type, order_id (int)} — что отвечает на законченный ордер, не
//!   сказано: ответ отмены не разбирается, итог — повторным опросом.

use std::time::{Duration, Instant};

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::errors::Op;
use super::order_result::{fee0, from_deals, from_state, result, unknown};
use super::order_rules::Prepared;
use super::{deals, num, rest};

/// Окончательные статусы ордера (order_status).
const FINAL: [&str; 3] = ["filled", "part_canceled", "canceled"];

/// Сроки ожидания: venue.order_poll_ms и venue.order_final_wait_ms; venue.my_trades_limit нужен поиску по сделкам.
/// Ошибка NotConfigured — не задана.
pub fn times(ctx: &Ctx) -> GatewayResult<(Duration, Duration)> {
    ctx.int("venue.my_trades_limit")?;
    Ok((
        Duration::from_millis(ctx.int("venue.order_poll_ms")?),
        Duration::from_millis(ctx.int("venue.order_final_wait_ms")?),
    ))
}

/// Что делает: окончательный итог ордера. Принимает: запрос ядра, подготовленный ордер, ответ постановки (order_id,
/// created_at, filled_amount). Ошибки: OutcomeUnknown — итог не получен ни опросом, ни по сделкам; NotConfigured.
pub async fn settle(
    ctx: &Ctx,
    req: TradeRequest,
    p: &Prepared,
    placed: &Value,
) -> GatewayResult<TradeResult> {
    let (poll, wait) = times(ctx)?;
    let id = num::text_opt(placed, "order_id").unwrap_or_default();
    if let Ok(s) = poll_final(ctx, &p.symbol, &id, poll, wait).await {
        return from_state(ctx, req, p, &s, id, wait).await;
    }
    let cancel = cancel(ctx, &p.symbol, &id).await;
    let why = match poll_final(ctx, &p.symbol, &id, poll, wait).await {
        Ok(s) => return from_state(ctx, req, p, &s, id, wait).await,
        Err(w) => w,
    };
    // Ордер не найден: ордер без исполнения CoinEx не хранит — итог по сделкам.
    let start = num::int(placed, "created_at").ok();
    let deadline = Instant::now() + wait;
    match deals::of_order(ctx, &p.symbol, &id, start, deadline).await {
        Ok(sum) if sum.count == 0 && put_zero(placed) => Ok(result(
            req,
            p.qty,
            Decimal::ZERO,
            Decimal::ZERO,
            fee0(p),
            id,
        )),
        Ok(sum) if sum.count > 0 => from_deals(ctx, req, p, &sum, id),
        Ok(_) => Err(unknown(
            ctx,
            &id,
            "ордер не найден, сделок нет, а ответ постановки не показал нулевое исполнение",
        )),
        Err(e) => {
            let c = cancel
                .err()
                .map(|e| format!("отмена: {}; ", e.message))
                .unwrap_or_default();
            Err(unknown(
                ctx,
                &id,
                &format!("{c}{why}; сделки: {}", e.message),
            ))
        }
    }
}

/// Ответ постановки показал нулевое исполнение (filled_amount = 0). Ошибок нет.
fn put_zero(placed: &Value) -> bool {
    matches!(num::dec_opt(placed, "filled_amount"), Ok(Some(f)) if f.is_zero())
}

/// Опрос до окончательного статуса не дольше wait; сбой запроса — повтор. Ошибка — текст последней причины.
async fn poll_final(
    ctx: &Ctx,
    market: &str,
    id: &str,
    poll: Duration,
    wait: Duration,
) -> Result<Value, String> {
    let started = Instant::now();
    loop {
        let why = match query(ctx, market, id).await {
            Ok(s) => {
                let st = num::text_opt(&s, "status").unwrap_or_default();
                if FINAL.contains(&st.as_str()) {
                    return Ok(s);
                }
                format!("ордер ещё не закончен (status = {st})")
            }
            Err(e) => format!("запрос ордера: {}", e.message),
        };
        if started.elapsed() >= wait {
            return Err(why);
        }
        tokio::time::sleep(poll).await;
    }
}

/// Отмена по order_id. Ошибки: order_id не целое, коды CoinEx, связь.
async fn cancel(ctx: &Ctx, market: &str, id: &str) -> GatewayResult<Value> {
    let oid: u64 = id
        .parse()
        .map_err(|_| super::errors::bad(&format!("order_id «{id}» не целое"), id))?;
    let body = json!({ "market": market, "market_type": "SPOT", "order_id": oid });
    rest::signed(
        ctx,
        Method::POST,
        "/v2/spot/cancel-order",
        vec![],
        Some(body),
        Op::Trade,
    )
    .await
}

/// Запрос ордера. Ошибки: как у rest::signed (в том числе «не сохранён»).
pub async fn query(ctx: &Ctx, market: &str, id: &str) -> GatewayResult<Value> {
    let q = vec![("market", market.to_string()), ("order_id", id.to_string())];
    rest::signed(
        ctx,
        Method::GET,
        "/v2/spot/order-status",
        q,
        None,
        Op::Other,
    )
    .await
}
