//! Файл: venues/bingx/resolve.rs
//! Что делает: решение 231 — исход операции «неизвестен» по «чем найти» из ошибки (gateway/pending.rs): ордер — по ID
//! ордера, если был, иначе по своему newClientOrderId (Query Order details по clientOrderID; не найден — None, ядро
//! спросит ещё), затем обычное доведение (order_state::settle: ждёт, отменяет остаток) и итог (order_state::result);
//! вывод — по своему withdrawOrderId в истории выводов. Свой ID у каждой постановки и вывода свой (решение 114,
//! transfers.rs) — поиск однозначен (открытые вопросы решения 63).
//! Кто вызывает: mod.rs (Venue::resolve_trade, Venue::resolve_withdraw) — ядро core/outcome_resolve.rs.
//! От чего зависит: rest.rs, num.rs, order_state.rs, ctx.rs, errors.rs, gateway/pending.rs.
//! Настройки: через order_state.rs (venue.order_poll_ms, venue.order_final_wait_ms).
//!
//! Официальная документация (api-ai-skills, https://github.com/bingx-api/api-ai-skills):
//! - spot-trade/api-reference.md «GET /openApi/spot/v1/trade/query — Query Single Order», «Rate limit: 10/s per UID»:
//!   symbol*, orderId, clientOrderID («Customized order ID for users, with a limit of characters from 1 to 40. Different
//!   orders cannot use the same clientOrder[ID]»); ответ data — orderId, status, executedQty, cummulativeQuoteQty, fee,
//!   feeAsset, clientOrderID.
//! - spot-wallet/api-reference.md «GET /openApi/api/v3/capital/withdraw/history», «Rate limit: 10 requests/s per UID»:
//!   id, coin, withdrawOrderId («Custom ID…»), …; ответ — голый массив записей с id и withdrawOrderId.

use reqwest::Method;
use serde_json::Value;

use crate::core::types::{TradeRequest, TradeResult, Transfer, WithdrawRequest};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::{self, PendingRef};

use super::ctx::Ctx;
use super::errors::{self, Op};
use super::{num, order_state, rest};

/// Что делает: окончательный итог ордера. Принимает: окружение, исходный запрос, «чем найти».
/// Возвращает: Some(итог); None — ордер не найден по clientOrderID. Ошибки: нет символа или своего ID (BadResponse),
/// ответ BingX на запрос (ядро спросит ещё), итог не получен (OutcomeUnknown — ядро спросит ещё).
pub async fn trade(
    ctx: &Ctx,
    req: TradeRequest,
    p: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let (Some(symbol), Some(client)) = (p.symbol.as_deref(), p.client_id.as_deref()) else {
        return Err(errors::bad(
            "нет символа или своего ID ордера",
            &format!("{p:?}"),
        ));
    };
    let order_id = match &p.venue_ref {
        Some(id) => id.clone(),
        None => {
            let q = vec![
                ("symbol", symbol.to_string()),
                ("clientOrderID", client.to_string()),
            ];
            let data = rest::signed(
                ctx,
                Method::GET,
                "/openApi/spot/v1/trade/query",
                q,
                Op::Other,
            )
            .await?;
            match order_of(&data, client) {
                Some(id) => id,
                None => return Ok(None),
            }
        }
    };
    let state = order_state::settle(ctx, symbol, &order_id).await?;
    order_state::result(ctx, req, &state, order_id).map(Some)
}

/// Что делает: создан ли вывод. Принимает: окружение, исходный запрос, «чем найти». Возвращает: Some(перевод Pending
/// с ID BingX); None — не найден. Ошибки: нет своего ID (BadResponse), ответ BingX (ядро спросит ещё).
pub async fn withdraw(
    ctx: &Ctx,
    req: WithdrawRequest,
    p: &PendingRef,
) -> GatewayResult<Option<Transfer>> {
    let Some(client) = p.client_id.as_deref() else {
        return Err(errors::bad("нет своего ID вывода", &format!("{p:?}")));
    };
    let q = vec![
        ("coin", req.asset.ticker.clone()),
        ("withdrawOrderId", client.to_string()),
    ];
    let path = "/openApi/api/v3/capital/withdraw/history";
    let data = rest::signed(ctx, Method::GET, path, q, Op::Other).await?;
    Ok(withdraw_of(&data, client).map(|id| pending::created(ctx.venue(), req, id)))
}

/// ID ордера из ответа Query Order details, если это ордер с нашим clientOrderID. Ошибок нет.
pub fn order_of(data: &Value, client: &str) -> Option<String> {
    let same = num::text_opt(data, "clientOrderID")
        .or_else(|| num::text_opt(data, "clientOrderId"))
        .is_none_or(|c| c == client);
    same.then(|| num::text_opt(data, "orderId")).flatten()
}

/// ID вывода с нашим withdrawOrderId из истории выводов (голый массив). Фильтр запроса не единственная проверка —
/// запись сверяется по полю. Ошибок нет.
pub fn withdraw_of(data: &Value, client: &str) -> Option<String> {
    data.as_array()?
        .iter()
        .find(|r| num::text_opt(r, "withdrawOrderId").as_deref() == Some(client))
        .and_then(|r| num::text_opt(r, "id"))
}
