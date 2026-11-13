//! Файл: venues/okx/trade.rs
//! Что делает: покупка или продажа на OKX лимитным ордером типа ioc / fok с худшей допустимой ценой (что не исполнилось
//! сразу — снимает биржа); затем окончательное состояние и факт (количество, средняя цена, комиссия и её монета —
//! order_result.rs). Итог неизвестен — с «чем найти» (PendingRef, решение 231; позднее выяснение — resolve.rs).
//! Правила — order_rules.rs (та же функция у check_order, решение 134).
//! Кто вызывает: mod.rs (trade).
//! От чего зависит: order_rules.rs, order_state.rs, order_result.rs, gateway/pending.rs, rest.rs, symbols.rs, num.rs,
//! ctx.rs.
//! Настройки: через order_rules.rs и order_state.rs.
//!
//! Официально — «POST / Place order» POST /api/v5/trade/order https://www.okx.com/docs-v5/en/: ответ data[] — ordId,
//! clOrdId, sCode («The code of the event execution result, 0 means success»), sMsg. Ответ без ordId при успешном коде
//! — итог неизвестен (повтор дал бы второй ордер).

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::pending::PendingRef;

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_result, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ OKX — sCode/sMsg),
/// OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    place(ctx, req, p).await
}

/// Что делает: постановка уже подготовленного ордера и его итог (вынесено из trade без изменений — решение 268: тот же
/// путь у рыночной продажи, market_sell.rs). Принимает: окружение, запрос ядра, ордер после правил (параметры запроса).
/// Возвращает и ошибки: как trade.
pub(super) async fn place(
    ctx: &Ctx,
    req: TradeRequest,
    p: order_rules::Prepared,
) -> GatewayResult<TradeResult> {
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
    // Решение 231 (264): «чем найти» — к любому «итог неизвестен» ниже (ядро выяснит исход само, resolve.rs).
    let pending = PendingRef::order(&p.symbol, &p.cl_ord_id, p.qty);
    let order_id = match placed {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.symbol, &p.cl_ord_id).await {
                Some(id) => id,
                None => return Err(e.pending(pending)),
            }
        }
        Err(e) => return Err(e),
    };
    let out = match order_state::settle(ctx, &p.symbol, &order_id).await {
        Ok(state) => order_result::result(ctx, req, &state, order_id.clone()),
        Err(e) => Err(e),
    };
    out.map_err(|e| e.pending(pending.with_ref(&order_id)))
}
