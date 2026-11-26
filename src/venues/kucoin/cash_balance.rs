//! Файл: venues/kucoin/cash_balance.rs.
//! Назначение: считывает ID и доступные деньги одного нативного торгового счёта KuCoin.
//! Потребители: общий вход CEX-покупки и опубликованные баланс-события приватной подписки.
//! Зависимости: существующие REST/WS аккаунта, фактическая такса пары и ledger.sqlite.
//! Настройки: текущие ключи и подписки адаптера.
use super::{account, cash_receipt, ctx::Ctx, fees::Fees, num};
use crate::{
    core::types::{Side, TradeRequest},
    gateway::{
        errors::{ErrorCode, GatewayError, GatewayResult},
        pending::PendingRef,
    },
    storage::cex_funds::{Balance, Bucket},
};
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::sync::broadcast;
#[derive(Clone)]
pub struct Feed(broadcast::Sender<Value>);
impl Default for Feed {
    fn default() -> Self {
        Self(broadcast::channel(32).0)
    }
}
impl Feed {
    pub fn observe(&self, data: &Value) {
        if self.0.receiver_count() > 0 {
            let _ = self.0.send(data.clone());
        }
    }
}
pub fn cap(
    ctx: &Ctx,
    fees: &Fees,
    req: &TradeRequest,
    p: &PendingRef,
    currency: &str,
) -> GatewayResult<Decimal> {
    let bad = |msg: &str| ctx.tag(GatewayError::new(ErrorCode::BadResponse, None, msg));
    let symbol = p
        .symbol
        .as_deref()
        .ok_or_else(|| bad("Нет исходного символа"))?;
    if req.side != Side::Buy || symbol != format!("{}-{currency}", req.asset.ticker) {
        return Err(bad(
            "Расчётная валюта или пара подготовленного ордера KuCoin изменилась",
        ));
    }
    let fee = fees.get(ctx, symbol)?.taker;
    if fee < Decimal::ZERO {
        return Err(bad("Отрицательная ставка taker KuCoin"));
    }
    let cap = p
        .sent_qty
        .and_then(|qty| qty.checked_mul(req.worst_price))
        .and_then(|q| q.checked_mul(Decimal::ONE.checked_add(fee)?))
        .ok_or_else(|| bad("Предел списания KuCoin вне диапазона"))?;
    Ok(cap)
}
pub async fn read(
    ctx: &Ctx,
    fees: &Fees,
    feed: &Feed,
    req: &TradeRequest,
    p: &PendingRef,
    currency: &str,
) -> GatewayResult<(Balance, Decimal)> {
    let bad = |msg: &str| ctx.tag(GatewayError::new(ErrorCode::BadResponse, None, msg));
    let cap = cap(ctx, fees, req, p, currency)?;
    let mut rx = feed.0.subscribe();
    let (bucket, free, locked) = account_snapshot(ctx, currency).await?;
    // Серия относится только к этому свежему запросу. Задержавшийся более старый кадр не
    // переносит своё доказательство на другой доступный остаток или иной accountId.
    let mut last = None;
    loop {
        match rx.try_recv() {
            Ok(d)
                if num::text_opt(&d, "accountId").as_deref() == Some(&bucket.account)
                    && num::text_opt(&d, "currency").as_deref() == Some(currency) =>
            {
                last = Some(d)
            }
            Ok(_) => {}
            Err(broadcast::error::TryRecvError::Lagged(_)) => {
                last = None;
                break;
            }
            Err(_) => break,
        }
    }
    let basis = Balance {
        bucket: bucket.clone(),
        free,
        epoch: 0,
        booked: vec![],
    };
    if let Some(d) = last
        && num::dec_opt(&d, "available")? == Some(free)
        && num::dec_opt(&d, "hold")? == Some(locked)
        && let Some(order) = d
            .pointer("/relationContext/orderId")
            .and_then(|v| v.as_str())
        && let Some((original, debit)) =
            crate::storage::cex_funds_original::terminal(ctx.journal.db().ledger(), &bucket, order)
                .await
                .map_err(|e| bad(&e))?
        && let Some(symbol) = original.lookup.symbol.as_deref()
        && let Some(r) = cash_receipt::single_fill(&d, &bucket, symbol, order, debit, 0)
    {
        record(ctx, &d, &r, order, debit).await?;
        return Ok((r.balance, cap));
    }
    Ok((basis, cap))
}

pub async fn account_snapshot(
    ctx: &Ctx,
    currency: &str,
) -> GatewayResult<(Bucket, Decimal, Decimal)> {
    let bad = |msg: &str| ctx.tag(GatewayError::new(ErrorCode::BadResponse, None, msg));
    let rows = account::trade_balances(ctx, Some(currency)).await?;
    let mut hit = rows
        .as_array()
        .ok_or_else(|| bad("Нет списка торговых счетов"))?
        .iter()
        .filter(|r| {
            num::text_opt(r, "type").as_deref() == Some("trade")
                && num::text_opt(r, "currency").as_deref() == Some(currency)
        });
    let row = hit
        .next()
        .ok_or_else(|| bad("Не найден исходный торговый счёт KuCoin"))?;
    if hit.next().is_some() {
        return Err(bad("Несколько подходящих торговых счетов"));
    }
    let bucket = Bucket {
        venue: ctx.venue().0,
        account: num::text(row, "id")?,
        currency: currency.into(),
    };
    let free = num::dec(row, "available")?;
    let locked = num::dec(row, "holds")?;
    let total = num::dec(row, "balance")?;
    if free < Decimal::ZERO || locked < Decimal::ZERO || free.checked_add(locked) != Some(total) {
        return Err(bad("Не сходится доступный и удержанный баланс KuCoin"));
    }
    Ok((bucket, free, locked))
}

/// Приватный поток может принести событие после REST-ответа IOC. В обычном режиме
/// нет запросов в SQLite; вообще чужие/неденежные события не пишутся.
pub async fn settled_event(ctx: &Ctx, data: &Value) -> GatewayResult<()> {
    let db = ctx.journal.db().ledger();
    if !db.cash_watch.unresolved(super::ctx::ID) {
        return Ok(());
    }
    if !matches!(
        num::text_opt(data, "relationEvent").as_deref(),
        Some("trade.setted" | "trade_hf.setted")
    ) {
        return Ok(());
    }
    let (Some(account), Some(currency), Some(order)) = (
        num::text_opt(data, "accountId"),
        num::text_opt(data, "currency"),
        data.get("relationContext")
            .and_then(|c| num::text_opt(c, "orderId")),
    ) else {
        return Ok(());
    };
    let b = Bucket {
        venue: super::ctx::ID.into(),
        account,
        currency,
    };
    if let Some((o, net)) = crate::storage::cex_funds_original::terminal(db, &b, &order)
        .await
        .map_err(|e| ctx.tag(GatewayError::new(ErrorCode::Internal, None, e)))?
        && let Some(symbol) = o.lookup.symbol.as_deref()
        && let Some(r) = cash_receipt::single_fill(data, &b, symbol, &order, net, 0)
    {
        record(ctx, data, &r, &order, net).await?;
    }
    Ok(())
}
async fn record(
    ctx: &Ctx,
    data: &Value,
    r: &cash_receipt::Receipt,
    order: &str,
    debit: Decimal,
) -> GatewayResult<()> {
    let source = serde_json::json!({"source":"/account/balance","event":r.event,"trade":r.trade,"symbol":r.symbol,
        "available":r.balance.free,"total":data.get("total"),"hold":data.get("hold"),"availableChange":data.get("availableChange"),"holdChange":data.get("holdChange")});
    crate::storage::cex_funds_original::observed(
        ctx.journal.db().ledger(),
        &r.balance.bucket,
        order,
        debit,
        source,
    )
    .await
    .map_err(|e| ctx.tag(GatewayError::new(ErrorCode::Internal, None, e)))
}
