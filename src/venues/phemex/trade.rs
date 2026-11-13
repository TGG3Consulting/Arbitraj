//! Файл: venues/phemex/trade.rs
//! Что делает: покупка или продажа на Phemex лимитным ордером с худшей допустимой ценой и сроком ImmediateOrCancel /
//! FillOrKill (что не исполнилось сразу — снимает биржа); затем окончательное состояние и факт (количество, сумма,
//! комиссия и её монета — order_state.rs). Правила — order_rules.rs (та же функция у check_order, решение 134).
//! Кто вызывает: mod.rs (trade).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, symbols.rs, ctx.rs.
//! Настройки: через order_rules.rs и order_state.rs.
//!
//! Официально — Place order (HTTP PUT, prefered) PUT /spot/orders/create?… (https://phemex-docs.github.io/
//! #place-order-http-put-prefered), группа лимитов SpotOrder (500/мин): ответ data — ордер со статусом «Created» и
//! bizError; это приём ордера, не исполнение (итог — Query orders by client order ID). HTTP 5XX — «execution status is
//! UNKNOWN» (General API information): ордер ищется по своему clOrdID до venue.order_final_wait_ms (order_state::settle;
//! отмены нет — IOC/FOK отменить нельзя, коды 11109/11110, решение 264); не найден — «итог неизвестен» (решение 64).
//! Повтора постановки нет.
//! Решение 264 — ответ постановки читается: bizError (пример ответа — 0; ненулевой — код таблицы BizError,
//! https://phemex-docs.github.io/#error-codes) и ordStatus «Rejected» (Common order fields) — отказ сразу, без опроса.
//! cumBaseQtyEv, cumQuoteQtyEv, avgPriceEp, cumFeeEv ответа итогом не берутся: описания полей ответа в офдоке нет, а
//! ordStatus примера — «Created» (не окончательный, в перечне Order status его нет); итог — Query orders by client
//! order ID. Сырой ответ пишется на диск (rest.rs) — сверка на живом ордере.

use reqwest::Method;
use serde_json::Value;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{errors, num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ Phemex),
/// OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    place(ctx, symbols, req, p).await
}

/// Что делает: постановка уже подготовленного ордера и его итог (вынесено из trade без изменений — решение 268: тот же
/// путь у рыночной продажи, market_sell.rs). Принимает: окружение, справочник пар (масштабы), запрос ядра, ордер после правил (параметры запроса).
/// Возвращает и ошибки: как trade.
pub(super) async fn place(
    ctx: &Ctx,
    symbols: &Symbols,
    req: TradeRequest,
    p: order_rules::Prepared,
) -> GatewayResult<TradeResult> {
    let path = "/spot/orders/create";
    let placed = rest::signed(ctx, Method::PUT, path, p.query.clone(), None, Op::Trade).await;
    match placed {
        Ok(d) => accepted(&d)?,
        // Ордер мог быть принят — ищем по своему clOrdID (settle); не найден — OutcomeUnknown оттуда же.
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            let m = format!(
                "{}: {} — ищем ордер по clOrdID {}",
                p.symbol, e.message, p.cl_ord_id
            );
            ctx.log(Level::Warn, "venue.trade", m).await;
        }
        Err(e) => return Err(e),
    }
    let state = order_state::settle(ctx, &p).await?;
    order_state::result(ctx, req, &state, &p, &symbols.scales())
}

/// Что делает: проверка ответа постановки (решение 264): bizError не 0 — отказ по таблице BizError (errors::from_code:
/// «не хватает средств» — InsufficientBalance, иначе OrderRejected); ordStatus «Rejected» — OrderRejected.
/// Принимает: data ответа постановки. Ошибки: перечисленные отказы. Поля нет — не отказ (итог — опросом).
pub fn accepted(data: &Value) -> Result<(), GatewayError> {
    let biz = num::text_opt(data, "bizError").unwrap_or_else(|| "0".to_string());
    let status = num::text_opt(data, "ordStatus").unwrap_or_default();
    if biz == "0" && status != "Rejected" {
        return Ok(());
    }
    let msg = format!("ордер не принят: bizError {biz}, ordStatus {status}");
    Err(errors::from_code(
        200,
        &biz,
        &msg,
        Op::Trade,
        &data.to_string(),
    ))
}
