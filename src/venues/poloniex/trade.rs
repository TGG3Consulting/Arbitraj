//! Файл: venues/poloniex/trade.rs
//! Что делает: покупка или продажа на Poloniex лимитным ордером с худшей допустимой ценой и сроком IOC/FOK (что не
//! исполнилось сразу — снимает биржа); затем окончательное состояние и факт (количество, средняя цена, комиссия и её
//! монета — order_state.rs, fills.rs). Правила — order_rules.rs (та же функция у check_order, решение 134).
//! Кто вызывает: mod.rs (trade).
//! Решение 268: тот же путь постановки и итога — у рыночной продажи (market_sell.rs передаёт готовый ордер, prepared).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, symbols.rs, num.rs, ctx.rs.
//! Настройки: через order_rules.rs и order_state.rs.
//!
//! Официально — Create Order POST /orders («50» на VIP0) https://api-docs.poloniex.com/spot/api/private/order: ответ
//! {id, clientOrderId} — это приём ордера, не исполнение (итог — Order Details). Ответ без id при успешном HTTP — итог
//! неизвестен (повтор дал бы второй ордер).
//! Решение 263 (Тигран 13.11): cancelReason из Order Details (там же, «code specified to provide reason for cancelation
//! if applicable») — в журнал venue.order (cancel_note); логика исполнения не меняется. 1004 «Due to self-trade» —
//! stpMode у нас не шлётся, по офдоке он «Defaults to EXPIRE_TAKER» (наш IOC снимается при встрече со своим ордером).

use reqwest::Method;
use serde_json::Value;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ Poloniex),
/// OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
/// Решение 268: `prepared` — уже подготовленный ордер (рыночная продажа, market_sell.rs): постановка и итог — тем же
/// кодом; None — обычный лимитный ордер по правилам order_rules::ready.
pub async fn trade(
    ctx: &Ctx,
    symbols: &Symbols,
    req: TradeRequest,
    prepared: Option<order_rules::Prepared>,
) -> GatewayResult<TradeResult> {
    let p = match prepared {
        Some(p) => p,
        None => order_rules::ready(ctx, symbols, &req).await?,
    };
    let placed = rest::signed(
        ctx,
        Method::POST,
        "/orders",
        vec![],
        Some(p.body),
        Op::Trade,
    )
    .await;
    let placed = placed.and_then(|r| {
        num::text_opt(&r, "id").ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "{}: ордер принят без id в ответе — итог неизвестен",
                    p.symbol
                ),
            ))
            .with_raw(r.to_string())
        })
    });
    let order_id = match placed {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.client_id).await {
                Some(id) => id,
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let state = order_state::settle(ctx, &order_id).await?;
    if let Some((level, m)) = cancel_note(&p.symbol, &order_id, &state) {
        ctx.log(level, "venue.order", m).await;
    }
    order_state::result(ctx, req, &state, order_id).await
}

/// Что делает: решение 263 — строка журнала о причине снятия ордера по полю cancelReason (Order Details,
/// https://api-docs.poloniex.com/spot/api/private/order): 0 «Not applicable» и нет поля — строки нет; 1 «As requested by
/// user»; 1000–1999 «Due to breach of controls in matching engine» (1004 «Due to self-trade» — Warn, 1012 «exceed
/// slippage tolerance»); 2000 «Due to margin liquidation», 2001 «Due to margin threshold breach», 2002 «Due to symbol
/// marked as offline». Код в ответе — число или строка. Принимает: символ, id ордера, ответ Order Details.
/// Возвращает: уровень и текст; None — причины нет. Ошибок нет.
pub fn cancel_note(symbol: &str, order_id: &str, s: &Value) -> Option<(Level, String)> {
    let code = match s.get("cancelReason")? {
        Value::String(t) => t.trim().parse::<i64>().ok()?,
        v => v.as_i64()?,
    };
    let (level, why) = match code {
        0 => return None,
        1 => (
            Level::Info,
            "As requested by user — отмена по запросу с аккаунта",
        ),
        1004 => (
            Level::Warn,
            "Due to self-trade — встретил свой ордер на этом аккаунте (stpMode по умолчанию EXPIRE_TAKER)",
        ),
        1012 => (Level::Info, "exceed slippage tolerance"),
        1000..=1999 => (Level::Info, "Due to breach of controls in matching engine"),
        2000 => (Level::Info, "Due to margin liquidation"),
        2001 => (Level::Info, "Due to margin threshold breach"),
        2002 => (Level::Info, "Due to symbol marked as offline"),
        _ => (Level::Info, "код не описан в документации"),
    };
    let state = s.get("state").and_then(Value::as_str).unwrap_or_default();
    Some((
        level,
        format!(
            "Poloniex {symbol}: ордер {order_id} ({state}) снят биржей, cancelReason {code}: {why}"
        ),
    ))
}
