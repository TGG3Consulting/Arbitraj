//! Файл: venues/okx/trade.rs
//! Что делает: покупка или продажа на OKX лимитным ордером типа ioc / fok с худшей допустимой ценой (что не исполнилось
//! сразу — снимает биржа); затем окончательное состояние и факт (количество, средняя цена, комиссия и её монета —
//! order_state.rs). Правила — order_rules.rs (та же функция у check_order, решение 134).
//! Кто вызывает: mod.rs (trade).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, symbols.rs, num.rs, ctx.rs.
//! Настройки: через order_rules.rs и order_state.rs.
//!
//! Официально — «POST / Place order» POST /api/v5/trade/order https://www.okx.com/docs-v5/en/: ответ data[] — ordId,
//! clOrdId, sCode («The code of the event execution result, 0 means success»), sMsg. Ответ без ordId при успешном коде
//! — итог неизвестен (повтор дал бы второй ордер).

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ OKX — sCode/sMsg),
/// OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    let placed = rest::post(ctx, "/api/v5/trade/order", p.body, Op::Trade).await;
    let placed = placed.and_then(|d| {
        let row = rest::first(&d, "trade/order").ok();
        let s_code = row.and_then(|r| num::text_opt(r, "sCode"));
        if let Some(c) = s_code.filter(|c| c != "0") {
            let m = row
                .and_then(|r| num::text_opt(r, "sMsg"))
                .unwrap_or_default();
            return Err(super::errors::from_code(&c, &m, Op::Trade, &d.to_string()));
        }
        row.and_then(|r| num::text_opt(r, "ordId")).ok_or_else(|| {
            ctx.tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "{}: ордер принят без ordId в ответе — итог неизвестен",
                    p.symbol
                ),
            ))
            .with_raw(d.to_string())
        })
    });
    let order_id = match placed {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.symbol, &p.cl_ord_id).await {
                Some(id) => id,
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let state = order_state::settle(ctx, &p.symbol, &order_id).await?;
    order_state::result(ctx, req, &state, order_id)
}
