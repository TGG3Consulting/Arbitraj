//! Файл: venues/mexc/fills.rs
//! Что делает: комиссия исполненного ордера MEXC по всем его сделкам (GET /api/v3/myTrades с limit) со сверкой суммы
//! количеств сделок с executedQty ордера (решение 118, А-🔴1). Не сошлось (сделки ещё не видны) — повтор чтения до
//! venue.order_final_wait_ms; так и не сошлось — OutcomeUnknown с фактом, а не заниженная комиссия.
//! Кто вызывает: order_state.rs (result), trade.rs (my_trades_limit — проверка до отправки ордера).
//! От чего зависит: rest.rs, num.rs, ctx.rs, errors.rs.
//! Настройки: venue.my_trades_limit [mexc] (обязательна), venue.order_poll_ms, venue.order_final_wait_ms.
//!
//! Официальная документация: GET /api/v3/myTrades (SPOT_ACCOUNT_READ, вес IP 10)
//! https://www.mexc.com/api-docs/spot-v3/spot-account-trade/account-trade-list — symbol (YES), orderId (NO),
//! limit (NO; «Default 10; max 1000» — пересказ таблицы); ответ — массив: orderId, qty, commission, commissionAsset.
//! Без limit приходят только 10 сделок: у ордера по худшей цене сделок бывает больше — комиссия занижалась.

use std::time::{Duration, Instant};

use reqwest::Method;
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::ctx::Ctx;
use super::errors::Op;
use super::{num, rest};

/// Наибольший limit у myTrades — из официальной документации («max 1000»).
pub const MAX_MY_TRADES_LIMIT: u64 = 1000;

/// Что делает: читает и проверяет venue.my_trades_limit [mexc].
/// Возвращает: значение 1..=1000. Ошибки: NotConfigured — не задана или больше предела документации.
pub fn limit(ctx: &Ctx) -> GatewayResult<u64> {
    let n = ctx.int("venue.my_trades_limit")?;
    if n == 0 || n > MAX_MY_TRADES_LIMIT {
        return Err(ctx.tag(GatewayError::not_set(
            None,
            "venue.my_trades_limit",
            format!(
                "venue.my_trades_limit = {n}: у MEXC от 1 до {MAX_MY_TRADES_LIMIT} (myTrades, «max 1000»)"
            ),
        )));
    }
    Ok(n)
}

/// Сделки одного чтения: сумма qty, сумма commission, монета комиссии, сколько сделок ордера пришло.
struct Read {
    qty: Decimal,
    fee: Decimal,
    asset: String,
    count: usize,
    /// Комиссия пришла в разных монетах: «первая, другая».
    mixed: Option<String>,
}

/// Что делает: комиссия ордера — сумма commission его сделок — после сверки суммы qty с executedQty.
/// Принимает: окружение, символ, ID ордера MEXC, executedQty ордера.
/// Возвращает: (комиссия, монета комиссии).
/// Ошибки: OutcomeUnknown — сделки так и не сошлись с executedQty за venue.order_final_wait_ms или не читаются
/// (ордер исполнен, а факт не подтверждён: ручная проверка); BadResponse — комиссия в разных монетах.
pub async fn commission(
    ctx: &Ctx,
    symbol: &str,
    order_id: &str,
    executed: Decimal,
) -> GatewayResult<(Decimal, String)> {
    let poll = Duration::from_millis(ctx.int("venue.order_poll_ms")?);
    let wait = Duration::from_millis(ctx.int("venue.order_final_wait_ms")?);
    let started = Instant::now();
    let why = loop {
        let why = match read(ctx, symbol, order_id).await {
            // Разные монеты комиссии — факт, повтор его не изменит.
            Ok(r) if r.mixed.is_some() => {
                let m = r.mixed.unwrap_or_default();
                return Err(super::errors::bad(
                    &format!("ордер {order_id}: комиссия в разных монетах ({m})"),
                    "",
                ));
            }
            Ok(r) if r.qty == executed => return Ok((r.fee, r.asset)),
            Ok(r) => format!(
                "сумма qty сделок {} (сделок {}) не равна executedQty {executed}, комиссия по ним {} {}",
                r.qty, r.count, r.fee, r.asset
            ),
            Err(e) => format!("сделки не прочитаны: {}", e.message),
        };
        if started.elapsed() >= wait {
            break why;
        }
        tokio::time::sleep(poll).await;
    };
    Err(ctx.tag(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        format!(
            "Ордер {order_id} ({symbol}) исполнен на {executed}, но комиссия не подтверждена за venue.order_final_wait_ms: {why}"
        ),
    )))
}

/// Что делает: одно чтение myTrades по ордеру.
/// Принимает: окружение, символ, ID ордера. Возвращает: Read.
/// Ошибки: настройка limit, связь, коды MEXC, BadResponse — не массив или поле не число.
async fn read(ctx: &Ctx, symbol: &str, order_id: &str) -> GatewayResult<Read> {
    let p = vec![
        ("symbol", symbol.to_string()),
        ("orderId", order_id.to_string()),
        ("limit", limit(ctx)?.to_string()),
    ];
    let data = rest::signed(ctx, Method::GET, "/api/v3/myTrades", p, Op::Other).await?;
    let trades = data
        .as_array()
        .ok_or_else(|| super::errors::bad("myTrades не массив", &data.to_string()))?;
    let mut r = Read {
        qty: Decimal::ZERO,
        fee: Decimal::ZERO,
        asset: String::new(),
        count: 0,
        mixed: None,
    };
    for t in trades
        .iter()
        .filter(|t| num::text_opt(t, "orderId").as_deref() == Some(order_id))
    {
        let a = num::text_opt(t, "commissionAsset").unwrap_or_default();
        if !r.asset.is_empty() && !a.is_empty() && a != r.asset {
            r.mixed = Some(format!("{}, {a}", r.asset));
        }
        if r.asset.is_empty() {
            r.asset = a;
        }
        r.qty += num::dec(t, "qty")?;
        r.fee += num::dec_opt(t, "commission")?
            .unwrap_or(Decimal::ZERO)
            .abs();
        r.count += 1;
    }
    Ok(r)
}
