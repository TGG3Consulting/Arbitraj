//! Файл: venues/mexc/settle_order.rs
//! Что делает: расчётный круг USDC Base — ордер конверсии на MEXC (пара venue.quote_asset, например USDCUSDT):
//! сверка сохранённого символа заявки с парой справочника (matches) и чтение окончательного исполнения
//! сохранённого ордера только запросами GET (inspect) — без отмены и без новой постановки.
//! Кто вызывает: mod.rs (Venue::matches_settlement_order, Venue::inspect_settlement_order) — ядро
//! core/settlement_effect.rs, core/settlement_cash.rs, core/cash_order_finish.rs.
//! От чего зависит: symbols.rs (правила пары из exchangeInfo), order_state.rs (query, is_final, result), num.rs,
//! ctx.rs, gateway/pending.rs.
//! Настройки: venue.quote_asset [mexc] (котируемая монета справочника пар); через order_state.rs —
//! venue.order_poll_ms, venue.order_final_wait_ms, venue.my_trades_limit (fills.rs). Своих новых нет.
//!
//! Официальная документация:
//! - GET /api/v3/exchangeInfo https://www.mexc.com/api-docs/spot-v3/market-data-endpoints/exchange-information —
//!   symbol, baseAsset, quoteAsset (символ пары берётся из ответа, а не склеивается).
//! - GET /api/v3/order https://www.mexc.com/api-docs/spot-v3/spot-account-trade/query-order (то же —
//!   https://mexcdevelop.github.io/apidocs/spot_v3_en/#query-order): symbol, origClientOrderId | orderId; ответ —
//!   symbol, orderId, clientOrderId, origClientOrderId, Qty (в старом тексте origQty), executedQty,
//!   cumulativeQuoteQty (в старом тексте cummulativeQuoteQty), status, side. «Orders up to 7 days old can be queried
//!   through the API» — старше 7 суток ордер этим запросом не находится (исход остаётся на сверке человека).

use rust_decimal::Decimal;
use serde_json::Value;

use crate::core::types::{Side, TradeRequest, TradeResult};
use crate::gateway::errors::GatewayResult;
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::symbols::{SymbolRules, Symbols};
use super::{num, order_state};

/// Что делает: относится ли сохранённая заявка к расчётной паре asset/quote. Символ пары — из справочника
/// exchangeInfo (ключ — baseAsset, котируемая — venue.quote_asset), не склейка строк.
/// Принимает: правила пары монеты asset (None — пары нет в справочнике), котируемую монету справочника
/// (venue.quote_asset; None — не задана), символ заявки, котируемую монету расчётной пары.
/// Возвращает: true — символ заявки равен символу этой пары. Ошибок нет (нет данных — false).
pub fn symbol_matches(
    rules: Option<&SymbolRules>,
    configured_quote: Option<&str>,
    pending_symbol: Option<&str>,
    quote: &str,
) -> bool {
    match (rules, configured_quote, pending_symbol) {
        (Some(r), Some(q), Some(s)) => q == quote && r.symbol == s,
        _ => false,
    }
}

/// Что делает: matches для адаптера — правила пары из памяти справочника. Принимает: окружение, справочник, заявку,
/// тикер монеты, котируемую монету. Возвращает: как symbol_matches. Ошибок нет.
pub fn matches(ctx: &Ctx, symbols: &Symbols, p: &PendingRef, asset: &str, quote: &str) -> bool {
    let rules = symbols.get(ctx, asset).ok();
    let configured = ctx.text("venue.quote_asset").ok();
    symbol_matches(
        rules.as_ref(),
        configured.as_deref(),
        p.symbol.as_deref(),
        quote,
    )
}

/// Что делает: проверяет, что ответ GET /api/v3/order — тот самый сохранённый ордер и он окончательный.
/// Принимает: ответ, символ и свой ID заявки, сторону, отправленное количество, известный ID ордера MEXC.
/// Возвращает: Some(orderId) — тот же ордер в окончательном состоянии; None — не подтверждён. Ошибок нет.
pub fn same_order(
    s: &Value,
    (symbol, client): (&str, &str),
    side: Side,
    sent_qty: Option<Decimal>,
    known: Option<&str>,
) -> Option<String> {
    if !order_state::is_final(s) || num::text_opt(s, "symbol").as_deref() != Some(symbol) {
        return None;
    }
    // Свой ID в ответе: clientOrderId или origClientOrderId (оба поля есть в таблице ответа документации).
    let own = ["clientOrderId", "origClientOrderId"]
        .iter()
        .any(|f| num::text_opt(s, f).as_deref() == Some(client));
    let want = if side == Side::Buy { "BUY" } else { "SELL" };
    let side_ok = num::text_opt(s, "side").is_some_and(|v| v.eq_ignore_ascii_case(want));
    let qty = num::dec_opt(s, "Qty")
        .ok()
        .flatten()
        .or_else(|| num::dec_opt(s, "origQty").ok().flatten());
    let qty_ok = qty.zip(sent_qty).is_none_or(|(a, b)| a == b);
    let id = num::text_opt(s, "orderId")?;
    let id_ok = known.is_none_or(|k| k == id);
    (own && side_ok && qty_ok && id_ok).then_some(id)
}

/// Что делает: окончательное исполнение сохранённого ордера только запросами GET (ордер по orderId, если он
/// известен, иначе по origClientOrderId; комиссия — GET /api/v3/myTrades в order_state::result). Сумма котируемой
/// монеты (cumulativeQuoteQty) передаётся ядру через settlement_intent::note_gross внутри result.
/// Принимает: окружение, исходный запрос, сохранённую заявку. Возвращает: Some(итог) — ордер найден, тот же и
/// окончательный; None — не найден, не окончательный или не совпал. Ошибки: сбой запроса (ядро спросит ещё),
/// BadResponse/OutcomeUnknown разбора итога и комиссии (как у trade).
pub async fn inspect(
    ctx: &Ctx,
    req: TradeRequest,
    p: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let (Some(symbol), Some(client)) = (p.symbol.as_deref(), p.client_id.as_deref()) else {
        return Ok(None);
    };
    let by = match p.venue_ref.as_deref() {
        Some(id) => ("orderId", id),
        None => ("origClientOrderId", client),
    };
    let s = order_state::query(ctx, symbol, by).await?;
    let Some(id) = same_order(
        &s,
        (symbol, client),
        req.side,
        p.sent_qty,
        p.venue_ref.as_deref(),
    ) else {
        return Ok(None);
    };
    let qty = p.sent_qty.unwrap_or(req.qty);
    order_state::result(ctx, symbol, req, &s, id, qty)
        .await
        .map(Some)
}
