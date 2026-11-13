//! Файл: venues/kraken/trade.rs
//! Что делает: покупка или продажа на Kraken лимитным ордером со сроком IOC / FOK по худшей допустимой цене (что не
//! исполнилось сразу — снимает биржа); затем окончательное состояние и факт (количество, средняя цена, комиссия и её
//! монета — order_state.rs). Правила — order_rules.rs (та же функция у check_order, решение 134).
//! Кто вызывает: mod.rs (trade).
//! Решение 268: тот же путь постановки и итога — у рыночной продажи (market_sell.rs передаёт готовый ордер, prepared).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, symbols.rs, num.rs, ctx.rs.
//! Настройки: через order_rules.rs и order_state.rs.
//!
//! Официально — POST /0/private/AddOrder (OpenAPI spot-rest.yaml, схема add): result {descr {order}, txid [..]}.
//! Ответ без txid при пустом error — итог неизвестен (повтор дал бы второй ордер).

use serde_json::Value;

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ Kraken), OutcomeUnknown —
/// ордер мог быть принят, итог не получен (решение 64).
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
    let placed = rest::private(ctx, "AddOrder", p.form.clone(), Op::Trade).await;
    let placed = placed.and_then(|res| {
        res.get("txid")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                ctx.tag(GatewayError::new(
                    ErrorCode::OutcomeUnknown,
                    None,
                    format!(
                        "{}: ордер принят без txid в ответе — итог неизвестен",
                        p.symbol
                    ),
                ))
                .with_raw(res.to_string())
            })
    });
    let txid = match placed {
        Ok(id) => id,
        Err(e) if e.code == ErrorCode::OutcomeUnknown => {
            match order_state::recover(ctx, &p.cl_ord_id).await {
                Some(id) => id,
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    let state = order_state::settle(ctx, &txid).await?;
    order_state::result(ctx, req, &state, txid)
}
