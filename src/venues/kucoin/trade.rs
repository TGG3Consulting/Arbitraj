//! Файл: venues/kucoin/trade.rs
//! Что делает: покупка или продажа на KuCoin лимитным ордером с худшей допустимой ценой и сроком IOC/FOK (что не
//! исполнилось сразу — снимает биржа) через Add Order Sync; затем окончательное состояние и факт (количество, средняя
//! цена, комиссия и её монета — order_state.rs). Правила и пробный ордер — order_rules.rs (та же функция у
//! check_order, решение 134).
//! Кто вызывает: mod.rs (trade).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, symbols.rs, num.rs, ctx.rs.
//! Настройки: через order_rules.rs и order_state.rs.
//!
//! Официально — Add Order Sync POST /api/v1/hf/orders/sync (право Spot, пул Spot, вес 1)
//! https://www.kucoin.com/docs-new/api-3470170: «this interface will synchronously return the order information after
//! the order matching is completed»; ответ orderId, clientOid, status (open/done), dealSize, remainSize, canceledSize.
//! Ответ без orderId при успешном HTTP — итог неизвестен (повтор дал бы второй ордер).

use reqwest::Method;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::reply::Pool;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, пробный, отказ KuCoin),
/// OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    let path = "/api/v1/hf/orders/sync";
    let placed = rest::signed(
        ctx,
        Method::POST,
        path,
        vec![],
        Some(p.body),
        Op::Trade,
        Pool::Spot,
    )
    .await;
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
    let order_id = match placed {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.symbol, &p.client_oid).await {
                Some(id) => id,
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let state = order_state::settle(ctx, &p.symbol, &order_id).await?;
    order_state::result(ctx, req, &state, order_id)
}
