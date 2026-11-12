//! Файл: venues/okx/resolve.rs
//! Что делает: решение 231 (решение 264 для OKX) — исход ордера «неизвестен» по «чем найти» из ошибки
//! (gateway/pending.rs): по ordId, если был, иначе по своему clOrdId (order_state::recover: поиск; не найден — отмена
//! по clOrdId и поиск до venue.order_final_wait_ms), затем обычное доведение (order_state::settle) и итог
//! (order_result::result). Пара уже не live — запрос ордера по офдоке неприменим, исход выясняет человек.
//! Кто вызывает: mod.rs (Venue::resolve_trade) — ядро core/outcome_resolve.rs.
//! От чего зависит: order_state.rs, order_result.rs, symbols.rs, ctx.rs, errors.rs, gateway/pending.rs.
//! Настройки: через order_state.rs (venue.order_poll_ms, venue.order_final_wait_ms).
//!
//! Официально — «Order Book Trading → Trade → GET / Order details» GET /api/v5/trade/order
//! https://www.okx.com/docs-v5/en/: instId («Instrument ID, e.g. BTC-USDT Only applicable to live instruments»), ordId
//! или clOrdId («If the clOrdId is associated with multiple orders, only the latest one will be returned» — clOrdId у
//! нас новый у каждой попытки, order_rules.rs). «Order history (last 7 days)»: «The incomplete orders that have been
//! canceled are only reserved for 2 hours».

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::symbols::Symbols;
use super::{errors, order_result, order_state};

/// Что делает: окончательный итог ордера. Принимает: окружение, справочник пар (свежесть — у вызывающего), исходный
/// запрос, «чем найти». Возвращает: Some(итог); None — пока не найден (ядро спросит ещё). Ошибки: нет символа или
/// своего ID (BadResponse); пара не live (NotApplicable — запрос ордера неприменим, человек); итог не получен
/// (OutcomeUnknown — ядро спросит ещё).
pub async fn trade(
    ctx: &Ctx,
    symbols: &Symbols,
    req: TradeRequest,
    p: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let (Some(symbol), Some(client)) = (p.symbol.as_deref(), p.client_id.as_deref()) else {
        return Err(errors::bad(
            "нет символа или своего ID ордера",
            &format!("{p:?}"),
        ));
    };
    if let Err(why) = live(symbols, symbol) {
        return Err(ctx.tag(GatewayError::new(ErrorCode::NotApplicable, None, why)));
    }
    let order_id = match &p.venue_ref {
        Some(id) => id.clone(),
        None => match order_state::recover(ctx, symbol, client).await {
            Some(id) => id,
            None => return Ok(None),
        },
    };
    let state = order_state::settle(ctx, symbol, &order_id).await?;
    order_result::result(ctx, req, &state, order_id).map(Some)
}

/// Что делает: пара в справочнике в state live (условие запроса ордера по instId). Принимает: справочник, instId.
/// Ошибки: текст — пары нет в справочнике или state не live.
pub fn live(symbols: &Symbols, inst: &str) -> Result<(), String> {
    let all = symbols.all();
    match all.values().find(|r| r.symbol == inst) {
        Some(r) if r.state.split('/').next() == Some("live") => Ok(()),
        Some(r) => Err(format!(
            "OKX: пара {inst} в state/ruleType {} — запрос ордера GET /api/v5/trade/order «Only applicable to live \
             instruments», исход ордера проверяет человек",
            r.state
        )),
        None => Err(format!(
            "OKX: пары {inst} нет в справочнике — запрос ордера GET /api/v5/trade/order «Only applicable to live \
             instruments», исход ордера проверяет человек"
        )),
    }
}
