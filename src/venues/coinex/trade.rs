//! Файл: venues/coinex/trade.rs
//! Что делает: покупка или продажа на CoinEx ордером типа ioc/fok с худшей допустимой ценой (что не исполнилось сразу —
//! снимает биржа); затем окончательный итог (количество, средняя цена, комиссия и её монета — order_state.rs). Правила —
//! order_rules.rs (та же функция у check_order, решение 134).
//! Кто вызывает: mod.rs (trade).
//! От чего зависит: order_rules.rs, order_state.rs, rest.rs, symbols.rs, num.rs, ctx.rs.
//! Настройки: через order_rules.rs и order_state.rs.
//!
//! Официально — Place Order POST /spot/order (группа «Place & edit spot orders (30r/1s)»)
//! https://docs.coinex.com/api/v2/spot/order/http/put-order: ответ data {order_id, filled_amount, filled_value, base_fee,
//! quote_fee, discount_fee, created_at, …} — поля status в ответе нет, окончательный ли это итог, не сказано: итог —
//! order_state.rs. Ответ без order_id при code 0 — итог неизвестен (повтор дал бы второй ордер). Поиска ордера по своему
//! client_id у CoinEx нет; при неизвестном итоге — отмена по client_id (POST /spot/cancel-order-by-client-id {market,
//! market_type, client_id}, группа «Batch cancel spot orders (40r/1s)»,
//! https://docs.coinex.com/api/v2/spot/order/http/cancel-order-by-client-id). Решение 264: её ответ разбирается — data
//! массив элементов {code, data, message}, во вложенном data — ордер (order_id, client_id, filled_amount, filled_value,
//! base_fee, quote_fee, discount_fee, created_at …; пример документации). Элемент с code 0 и ордером своего client_id —
//! найден order_id: дальше обычное доведение (order_state::settle). Иначе — «итог неизвестен» с code/message элемента
//! (ручная проверка). Что отвечает отмена уже закрытого IOC, офдока не говорит — проверить на живом ответе.

use reqwest::Method;
use serde_json::{Value, json};

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::symbols::Symbols;
use super::{num, order_rules, order_state, rest};

/// Ставит ордер и возвращает фактический результат. Ошибки: OrderRejected (правила, отказ CoinEx),
/// OutcomeUnknown — ордер мог быть принят, итог не получен (решение 64).
pub async fn trade(ctx: &Ctx, symbols: &Symbols, req: TradeRequest) -> GatewayResult<TradeResult> {
    let p = order_rules::ready(ctx, symbols, &req).await?;
    let path = "/v2/spot/order";
    let placed = rest::signed(
        ctx,
        Method::POST,
        path,
        vec![],
        Some(p.body.clone()),
        Op::Trade,
    )
    .await;
    let placed = placed.and_then(|r| match num::text_opt(&r, "order_id") {
        Some(_) => Ok(r),
        None => Err(ctx
            .tag(GatewayError::new(
                ErrorCode::OutcomeUnknown,
                None,
                format!(
                    "{}: ордер принят без order_id в ответе — итог неизвестен",
                    p.symbol
                ),
            ))
            .with_raw(r.to_string())),
    });
    match placed {
        Ok(r) => order_state::settle(ctx, req, &p, &r).await,
        Err(mut e) if e.code == ErrorCode::OutcomeUnknown => {
            let body = json!({"market": p.symbol, "market_type": "SPOT", "client_id": p.client_id});
            let c = rest::signed(
                ctx,
                Method::POST,
                "/v2/spot/cancel-order-by-client-id",
                vec![],
                Some(body),
                Op::Other,
            )
            .await;
            // Решение 264: ответ отмены несёт ордер — свой найден: обычное доведение по order_id.
            let found = c
                .map_err(|x| x.message)
                .and_then(|d| cancelled(&d, &p.client_id));
            let why = match found {
                Ok(order) => return order_state::settle(ctx, req, &p, &order).await,
                Err(why) => why,
            };
            e.message = format!(
                "{} (отмена по client_id: {why}; client_id {})",
                e.message, p.client_id
            );
            Err(e)
        }
        Err(e) => Err(e),
    }
}

/// Что делает: ордер из ответа отмены по client_id (решение 264): data — массив {code, data, message}; берётся элемент с
/// code 0, у которого во вложенном data есть order_id и client_id равен своему (поле есть в таблице ответа).
/// Принимает: data ответа, свой client_id. Возвращает: объект ордера. Ошибка — текст: data не массив, своего ордера нет
/// (с code и message элементов, как прислал CoinEx).
pub fn cancelled(data: &Value, client_id: &str) -> Result<Value, String> {
    let list = data
        .as_array()
        .ok_or_else(|| format!("data ответа не массив: {data}"))?;
    let mut seen = Vec::new();
    for it in list {
        let code = num::text_opt(it, "code").unwrap_or_default();
        let order = it.get("data").unwrap_or(&Value::Null);
        let mine = num::text_opt(order, "client_id").as_deref() == Some(client_id);
        if code == "0" && mine && num::text_opt(order, "order_id").is_some() {
            return Ok(order.clone());
        }
        let msg = num::text_opt(it, "message").unwrap_or_default();
        seen.push(format!("code {code}: {msg}"));
    }
    Err(format!("своего ордера в ответе нет ({})", seen.join("; ")))
}
