//! Файл: venues/binance/settle.rs
//! Что делает: расчётный круг Base (USDC ↔ USDT) у Binance — сверка по сохранённому до отправки «чем найти»
//! (gateway/settlement_intent.rs): ордер конверсии — только чтением GET /api/v3/order по своему ID (origClientOrderId)
//! с проверкой символа, стороны и количества; вывод — по своему withdrawOrderId в истории выводов. Проверка, что
//! сохранённая заявка относится к расчётной паре — по справочнику пар (exchangeInfo: symbol пары монеты с котируемой
//! venue.quote_asset).
//! Кто вызывает: mod.rs (Venue::matches_settlement_order, inspect_settlement_order, resolve_withdraw) — ядро
//! core/settlement_effect.rs, core/settlement_withdrawal.rs.
//! От чего зависит: order_state.rs (query, is_final, result), transfers.rs (find_own), symbols.rs, num.rs, ctx.rs,
//! gateway/pending.rs.
//! Настройки: venue.quote_asset (символ пары); venue.withdraw_history_min_ms (история выводов, transfers.rs);
//! venue.my_trades_limit (комиссия по сделкам ордера, fills.rs). Новых настроек нет.
//!
//! Официальная документация:
//! - Query order (USER_DATA) GET /api/v3/order, вес 4 — https://developers.binance.com/docs/binance-spot-api-docs/rest-api/account-endpoints
//!   («Either orderId or origClientOrderId must be sent»; при обоих — orderId и проверка совпадения clientOrderId);
//!   поля symbol, orderId, clientOrderId, origQty, executedQty, cummulativeQuoteQty, status, side.
//! - Withdraw History (supporting network) GET /sapi/v1/capital/withdraw/history —
//!   https://developers.binance.com/docs/wallet/capital/withdraw-history (withdrawOrderId; поля id, coin, network,
//!   withdrawOrderId «Returned only if one was set»).
//! - Exchange information GET /api/v3/exchangeInfo — symbol, baseAsset, quoteAsset (symbols_parse.rs).

use crate::core::types::{Side, TradeRequest, TradeResult, Transfer, WithdrawRequest};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::{self, PendingRef};

use super::ctx::Ctx;
use super::symbols::Symbols;
use super::{errors, num, order_state, transfers};

/// Что делает: сохранённая заявка — ордер расчётной пары: её символ равен символу пары монеты `asset` из справочника
/// (exchangeInfo), а `quote` — котируемая монета справочника (venue.quote_asset). Принимает: окружение, справочник,
/// «чем найти», монету и котируемую монету ядра. Возвращает: true — та же пара. Ошибок нет (нет данных — false).
pub fn matches(ctx: &Ctx, symbols: &Symbols, p: &PendingRef, asset: &str, quote: &str) -> bool {
    let Some(saved) = p.symbol.as_deref() else {
        return false;
    };
    if ctx.text("venue.quote_asset").ok().as_deref() != Some(quote) {
        return false;
    }
    symbols
        .get(ctx, asset)
        .is_ok_and(|rules| rules.symbol == saved)
}

/// Что делает: окончательный итог сохранённого ордера конверсии только чтением (GET /api/v3/order по origClientOrderId;
/// если известен orderId — оба, Binance сам сверяет их совпадение). Ордер не окончательный, другой символ, другой свой
/// ID, другая сторона или количество — None (ядро сверит позже, ничего не отправляя). Принимает: окружение, запрос
/// ядра, «чем найти». Возвращает: Some(итог, с оборотом cummulativeQuoteQty для note_gross — order_state::result);
/// None — подтверждения пока нет. Ошибки: связь, коды Binance, разбор (как order_state::query / result).
pub async fn inspect(
    ctx: &Ctx,
    req: TradeRequest,
    p: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let (Some(symbol), Some(client)) = (p.symbol.as_deref(), p.client_id.as_deref()) else {
        return Ok(None);
    };
    let s = match p.venue_ref.as_deref() {
        Some(id) => {
            let q = vec![
                ("symbol", symbol.to_string()),
                ("orderId", id.to_string()),
                ("origClientOrderId", client.to_string()),
            ];
            let path = "/api/v3/order";
            super::rest::signed(ctx, reqwest::Method::GET, path, q, errors::Op::Other).await?
        }
        None => order_state::query(ctx, symbol, ("origClientOrderId", client)).await?,
    };
    let side = if req.side == Side::Buy { "BUY" } else { "SELL" };
    if !order_state::is_final(&s)
        || num::text_opt(&s, "symbol").as_deref() != Some(symbol)
        || num::text_opt(&s, "clientOrderId").as_deref() != Some(client)
        || num::text_opt(&s, "side").as_deref() != Some(side)
        || num::dec_opt(&s, "origQty")?
            .zip(p.sent_qty)
            .is_none_or(|(actual, sent)| actual != sent)
    {
        return Ok(None);
    }
    let Some(id) = num::text_opt(&s, "orderId") else {
        return Ok(None);
    };
    if p.venue_ref.as_ref().is_some_and(|known| known != &id) {
        return Ok(None);
    }
    let qty = p.sent_qty.unwrap_or(req.qty);
    order_state::result(ctx, symbol, req, &s, id, qty)
        .await
        .map(Some)
}

/// Что делает: создан ли вывод с потерянным ответом — по своему withdrawOrderId в истории выводов (последние 7 дней);
/// запись должна быть той же монеты и сети, что запрос. Принимает: окружение, исходный запрос, «чем найти».
/// Возвращает: Some(перевод Pending с id Binance) — дальше обычный опрос статуса по idList; None — записи пока нет.
/// Ошибки: нет своего ID (BadResponse); запись с этим ID другой монеты или сети (BadResponse — сверить вручную);
/// связь, коды, настройка venue.withdraw_history_min_ms (ядро спросит ещё).
pub async fn withdraw(
    ctx: &Ctx,
    req: WithdrawRequest,
    p: &PendingRef,
) -> GatewayResult<Option<Transfer>> {
    let Some(client) = p.client_id.as_deref() else {
        return Err(errors::bad("нет своего ID вывода", &format!("{p:?}")));
    };
    let Some(rec) = transfers::find_own(ctx, client).await? else {
        return Ok(None);
    };
    if num::text_opt(&rec, "coin").as_deref() != Some(req.asset.ticker.as_str())
        || num::text_opt(&rec, "network").as_deref() != Some(req.network.as_str())
    {
        return Err(errors::bad(
            &format!("вывод {client}: монета или сеть записи не совпадают с запросом"),
            &rec.to_string(),
        ));
    }
    Ok(num::text_opt(&rec, "id").map(|id| pending::created(ctx.venue(), req, id)))
}
