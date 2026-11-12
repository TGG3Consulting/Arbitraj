//! Файл: venues/coinex/deals.rs
//! Что делает: сделки одного своего ордера CoinEx (запрос по order_id, решение 264): сколько исполнено, на какую сумму,
//! комиссия и её монета. Это факт исполнения там, где ордер не найти: «canceled orders without execution will not be
//! saved» (ордер IOC без исполнения не сохраняется вовсе — нет сделок = не исполнился), и монета комиссии при скидке
//! (discount_fee).
//! Кто вызывает: order_state.rs, order_result.rs, tests/coinex_real_data.rs.
//! От чего зависит: rest.rs (signed_page), num.rs, ctx.rs.
//! Настройки: venue.my_trades_limit [coinex] — сделок на страницу (limit; по умолчанию у CoinEx 10, максимум не указан).
//!
//! Официально — Get User Order Deals GET /spot/order-deals?market=&market_type=SPOT&order_id=&page=&limit= (подпись)
//! https://docs.coinex.com/api/v2/spot/deal/http/list-user-order-deals: order_id обязателен; data[] {deal_id,
//! created_at, order_id, market, margin_market, side, price, amount «Filled volume», role, fee, fee_ccy};
//! pagination.has_next. Группа лимита на странице не названа. Решение 264: раньше читалась вся история сделок пары
//! (List User Deals GET /spot/user-deals с start_time = создание ордера) со своим фильтром — шапка утверждала «фильтра
//! по order_id у CoinEx нет», офдока это опровергает. Поле order_id каждой сделки всё равно сверяется (add).

use std::collections::BTreeMap;
use std::time::Instant;

use rust_decimal::Decimal;
use serde_json::Value;

use crate::gateway::errors::{GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::{num, rest};

/// Итог сделок ордера.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sum {
    /// Сделок найдено.
    pub count: usize,
    /// Исполнено (Σ amount).
    pub filled: Decimal,
    /// Сумма (Σ amount × price).
    pub value: Decimal,
    /// Комиссия по монетам (fee_ccy → Σ |fee|).
    pub fees: BTreeMap<String, Decimal>,
}

/// Что делает: сделки ордера order_id на рынке market (все страницы, пока has_next), не дольше deadline.
/// Возвращает: Sum. Ошибки: ключи, связь, коды CoinEx, поле не число; не успели прочитать все страницы — Timeout.
pub async fn of_order(
    ctx: &Ctx,
    market: &str,
    order_id: &str,
    deadline: Instant,
) -> GatewayResult<Sum> {
    let limit = ctx.int("venue.my_trades_limit")?;
    let mut sum = Sum::default();
    for page in 1u64.. {
        let q = vec![
            ("market", market.to_string()),
            ("market_type", "SPOT".to_string()),
            ("order_id", order_id.to_string()),
            ("page", page.to_string()),
            ("limit", limit.to_string()),
        ];
        let (data, next) = rest::signed_page(ctx, "/v2/spot/order-deals", q).await?;
        add(&mut sum, &data, order_id)?;
        if !next {
            return Ok(sum);
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    Err(ctx.tag(GatewayError::new(
        crate::gateway::errors::ErrorCode::Timeout,
        None,
        format!(
            "CoinEx {market}: сделки ордера {order_id} не дочитаны за venue.order_final_wait_ms"
        ),
    )))
}

/// Добавляет к итогу сделки страницы с этим order_id (сверка поля order_id каждой сделки). Ошибка BadResponse — data не массив, поле не число.
pub fn add(sum: &mut Sum, data: &Value, order_id: &str) -> GatewayResult<()> {
    let list = data
        .as_array()
        .ok_or_else(|| super::errors::bad("order-deals: data не массив", &data.to_string()))?;
    for d in list {
        if num::text_opt(d, "order_id").as_deref() != Some(order_id) {
            continue;
        }
        let amount = num::dec(d, "amount")?;
        sum.count += 1;
        sum.filled += amount;
        sum.value += amount * num::dec(d, "price")?;
        if let Some(fee) = num::dec_opt(d, "fee")?.filter(|f| !f.is_zero()) {
            let ccy = num::text(d, "fee_ccy")?.to_ascii_uppercase();
            *sum.fees.entry(ccy).or_default() += fee.abs();
        }
    }
    Ok(())
}
