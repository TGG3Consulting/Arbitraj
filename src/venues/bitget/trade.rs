//! Файл: venues/bitget/trade.rs
//! Что делает: покупка или продажа на Bitget лимитным ордером с худшей допустимой ценой и сроком ioc/fok (что не
//! исполнилось сразу — снимает биржа); затем окончательное состояние и факт (количество, средняя цена, комиссия и её
//! монета — order_state.rs, order_fee.rs). Правила — order_rules.rs (та же функция у check_order, решение 134).
//! Кто вызывает: mod.rs (trade).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, symbols.rs, num.rs, ctx.rs.
//! Настройки: через order_rules.rs и order_state.rs.
//!
//! Официально — Place Order POST /api/v2/spot/trade/place-order («10 times/1s (UID)»)
//! https://www.bitget.com/docs/catalog/classic-spot-trade/classic-spot-trade#place-order: ответ data {orderId,
//! clientOid} — это приём ордера, не исполнение (итог — Get Order Info). Ответ без orderId при успешном HTTP — итог
//! неизвестен (повтор дал бы второй ордер).

use reqwest::Method;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ Bitget),
/// OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    let path = "/api/v2/spot/trade/place-order";
    let placed = rest::signed(ctx, Method::POST, path, vec![], Some(p.body), Op::Trade).await;
    let placed = placed.and_then(|r| {
        num::text_opt(&r, "orderId").ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "{}: ордер принят без orderId в ответе — итог неизвестен",
                    p.symbol
                ),
            ))
            .with_raw(r.to_string())
        })
    });
    // Решение 231: «чем найти» — к любому «итог неизвестен» ниже (ядро выяснит исход само, outcome_resolve.rs).
    let pending = PendingRef::order(&p.symbol, &p.client_oid, req.qty).received_coin(&p.received);
    let order_id = match placed {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.symbol, &p.client_oid).await {
                Some(id) => id,
                None => return Err(e.pending(pending)),
            }
        }
        Err(e) => return Err(e),
    };
    let by_id = pending.with_ref(&order_id);
    let state = order_state::settle(ctx, &p.symbol, &order_id)
        .await
        .map_err(|e| e.pending(by_id.clone()))?;
    order_state::result(ctx, req, &state, order_id, &p.received).map_err(|e| e.pending(by_id))
}
