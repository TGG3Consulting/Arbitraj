//! Файл: venues/bingx/settle.rs
//! Что делает: расчётный круг USDC (сеть Base) — чтение сохранённого ордера конверсии USDC-USDT без отправки и без
//! отмены (контракт `inspect_settlement_order`) и сверка символа сохранённой заявки с расчётной парой
//! (`matches_settlement_order`). Ордер ставит обычный trade.rs (там же сохранение своего ID до POST через
//! gateway/settlement_intent.rs::before_send), точная сумма котируемой валюты — order_state::result (note_gross).
//! Кто вызывает: mod.rs (Venue::inspect_settlement_order, Venue::matches_settlement_order) — ядро
//! core/settlement_effect.rs, core/cash_order*.rs.
//! От чего зависит: rest.rs, num.rs, order_state.rs, ctx.rs, errors.rs, gateway/pending.rs.
//! Настройки: нет (только GET; сроки ожидания ордера здесь не нужны — открытый ордер остаётся на сверке).
//!
//! Официальная документация:
//! - docs-v3 (https://bingx-api.github.io/docs-v3/), Spot → Trades Endpoints → «Query Order details»
//!   GET /openApi/spot/v1/trade/query, «10/second»: symbol* («e.g., BTC-USDT»), orderId, clientOrderID («Customized
//!   order ID for users, with a limit of characters from 1 to 40. Different orders cannot use the same clientOrderID,
//!   Only supports a query range of 2 hours»); ответ data: symbol, orderId, origQty, executedQty, cummulativeQuoteQty
//!   («Cumulative quote asset transacted quantity»), status (NEW, PENDING, PARTIALLY_FILLED, FILLED, CANCELED, FAILED),
//!   side (BUY/SELL), fee, feeAsset, clientOrderID.
//! - Тот же раздел в api-ai-skills: https://github.com/BingX-API/api-ai-skills/blob/main/skills/spot-trade/api-reference.md
//! - Символ пары — «BASE-QUOTE» (docs-v3 Spot → Market Data → «Spot trading symbols»; живой ответ
//!   tests/data/bingx_2026-11-21/symbols.json: «USDC-USDT»).

use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, order_state, rest};

/// Что делает: относится ли сохранённая заявка к расчётной паре. Символ BingX — «<монета>-<котируемая>» (docs-v3,
/// «Spot trading symbols»). Принимает: «чем найти» сохранённой заявки, тикер монеты BingX, тикер котируемой.
/// Возвращает: true — символ заявки ровно «asset-quote»; нет символа или иной — false. Ошибок нет.
pub fn matches(p: &PendingRef, asset: &str, quote: &str) -> bool {
    !asset.is_empty()
        && !quote.is_empty()
        && p.symbol.as_deref() == Some(format!("{asset}-{quote}").as_str())
}

/// Что делает: окончательное исполнение сохранённого ордера — только GET «Query Order details»: по orderId, если он
/// сохранён, иначе по своему clientOrderID (документация: поиск по нему — только в пределах 2 часов). Ордер не
/// отменяется и не доводится: не окончательный — None (остаётся на сверке).
/// Принимает: окружение, исходный запрос ядра, «чем найти» (символ, свой ID, отправленное количество, orderId).
/// Возвращает: Some(итог, с точной суммой cummulativeQuoteQty для settlement_intent) — ответ окончательный и совпал
/// с заявкой (символ, свой ID, сторона, количество, orderId); None — нет реквизитов, не окончательный или не совпал.
/// Ошибки: ответ BingX на запрос (ядро спросит ещё), BadResponse — нет полей итога (order_state::result).
pub async fn inspect(
    ctx: &Ctx,
    req: TradeRequest,
    p: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let (Some(symbol), Some(client)) = (p.symbol.as_deref(), p.client_id.as_deref()) else {
        return Ok(None);
    };
    let by = match p.venue_ref.as_deref() {
        Some(id) => ("orderId", id.to_string()),
        None => ("clientOrderID", client.to_string()),
    };
    let q = vec![("symbol", symbol.to_string()), by];
    let s = rest::signed(
        ctx,
        Method::GET,
        "/openApi/spot/v1/trade/query",
        q,
        Op::Other,
    )
    .await?;
    let Some(id) = same_order(&s, p, req.side) else {
        return Ok(None);
    };
    order_state::result(ctx, req, &s, id).map(Some)
}

/// Что делает: проверка ответа «Query Order details» против сохранённой заявки. Принимает: data ответа, «чем найти»,
/// сторону запроса ядра. Возвращает: Some(orderId) — окончательный статус, тот же символ, тот же clientOrderID,
/// та же сторона (BUY/SELL), origQty равен отправленному количеству и orderId совпал с сохранённым (если был);
/// иначе None. Ошибок нет: любое расхождение оставляет ордер на сверке.
pub fn same_order(s: &Value, p: &PendingRef, side: Side) -> Option<String> {
    let client = p.client_id.as_deref()?;
    let sent: Decimal = p.sent_qty?;
    let got_client =
        num::text_opt(s, "clientOrderID").or_else(|| num::text_opt(s, "clientOrderId"));
    let side_text = if side == Side::Buy { "BUY" } else { "SELL" };
    let orig = num::dec_opt(s, "origQty").ok().flatten();
    let id = num::text_opt(s, "orderId")?;
    let ok = order_state::is_final(s)
        && num::text_opt(s, "symbol").as_deref() == p.symbol.as_deref()
        && got_client.as_deref() == Some(client)
        && num::text_opt(s, "side").as_deref() == Some(side_text)
        && orig == Some(sent)
        && p.venue_ref.as_deref().is_none_or(|known| known == id);
    ok.then_some(id)
}
