//! Файл: venues/bsc_dex/exec/trade_smart.rs
//! Что делает: сделка по умному маршруту из котировки (решение 38): пересчёт каждой доли по живому состоянию её
//! пула (перед этим — повторная проба крючка B/C, решение 32), проверка средней цены против худшей допустимой, минимум
//! выхода по каждой доле, ожидаемые поступления выходного токена (wallet/inflows.rs), сверка долей с MixedQuoter
//! PancakeSwap, разрешения и один execute Universal Router PancakeSwap со всеми долями и итоговой проверкой баланса
//! (exec/universal.rs), ожидание итога своим узлом, факт по логам Transfer. Повторов нет (стартовое правило Тиграна).
//! Кто вызывает: exec/trade.rs.
//! От чего зависит: exec/universal.rs, exec/trade.rs (общие шаги), exec/precheck.rs, exec/result.rs, quote/direct.rs, quote/select.rs,
//! quote/mixed_quoter.rs, wallet/nonce.rs, pools/registry.rs.
//! Настройки: через вызываемые — smart.*, exec.dex_slippage_pct, dex.*, send.*, channel.*.
//!
//! Количество доли: продажа — qty × доля входа (RouteLeg.share); покупка — qty × ожидаемые монеты доли / все
//! ожидаемые монеты; последняя доля получает остаток, чтобы сумма долей была ровно qty.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::registry::Entry;
use super::super::quote::mixed_quoter::{self, Leg};
use super::super::quote::{Quoting, direct, select};
use super::super::wallet::nonce::Nonces;
use super::plan::Order;
use super::trade::{check_price, order, send_plan, worst_positive};
use super::txs::Env;
use super::{precheck, result, universal};

/// Доля, пересчитанная по живому состоянию.
struct Live {
    entry: Entry,
    order: Order,
    pool_in: alloy_primitives::U256,
    pool_out: alloy_primitives::U256,
    hook_pct: Decimal,
}

/// Сделка по умному маршруту. Ошибки: NoRoute (доля не одна в пуле, пул пропал или не допущен), QuoteMismatch
/// (цена хуже допустимой, расхождение с MixedQuoter), CoinNotChecked, настройки, каналы,
/// SwapReverted / TxNotIncluded / NonceConflict (с фактом транзакции).
pub async fn trade(
    q: &Quoting,
    env: &Env,
    nonces: &Nonces,
    req: TradeRequest,
    route: &DexRoute,
) -> GatewayResult<TradeResult> {
    let ctx = &q.ctx;
    worst_positive(ctx, &req)?;
    let taxes = direct::taxes(q, &req.asset)?;
    let weights: Vec<Decimal> = route
        .legs
        .iter()
        .map(|l| {
            if req.side == Side::Buy {
                l.expected_out
            } else {
                l.share
            }
        })
        .collect();
    let total: Decimal = weights.iter().sum();
    if route.legs.is_empty() || total <= Decimal::ZERO {
        return Err(ctx.err(ErrorCode::NoRoute, "Умный маршрут без долей"));
    }
    let mut live = Vec::with_capacity(route.legs.len());
    let (mut coins, mut usdt, mut left) = (Decimal::ZERO, Decimal::ZERO, req.qty);
    let mut decs = (0u8, 0u8);
    let mut guard = None;
    for (i, (leg, w)) in route.legs.iter().zip(&weights).enumerate() {
        let [hop] = leg.hops.as_slice() else {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                "Доля умного маршрута — больше одного пула: хопы не поддержаны",
            ));
        };
        let e = q
            .reg
            .get(&hop.pool)
            .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "Пул маршрута больше не в реестре"))?;
        if i == 0 {
            let out_token = if req.side == Side::Buy {
                e.coin
            } else {
                e.usdt
            };
            guard = Some(env.inflows.swap(ctx, out_token).await?);
            decs = (
                q.tokens.decimals(ctx, e.coin).await?,
                q.tokens.decimals(ctx, e.usdt).await?,
            );
        }
        let qty = if i + 1 == route.legs.len() {
            left
        } else {
            (req.qty * w / total).min(left)
        };
        left -= qty;
        precheck::hook_again(q, &e).await?;
        let u = select::usable(ctx, &q.checks, &e, req.side, decs.1)
            .map_err(|why| ctx.err(ErrorCode::NoRoute, format!("{}: {why}", e.family.title)))?;
        let (s, c, x) = direct::calc(ctx, &e, &u, req.side, qty, &taxes, decs)?;
        coins += c;
        usdt += x;
        let o = order(ctx, &req, &e, qty, (c, x), decs)?;
        live.push(Live {
            entry: e,
            order: o,
            pool_in: s.amount_in,
            pool_out: s.amount_out,
            hook_pct: u.hook_pct,
        });
    }
    check_price(ctx, &req, coins, usdt)?;
    let legs: Vec<Leg<'_>> = live
        .iter()
        .map(|l| Leg {
            entry: &l.entry,
            token_in: l.order.token_in,
            token_out: l.order.token_out,
            amount_in: l.pool_in,
            local_out: l.pool_out,
            hook_pct: l.hook_pct,
        })
        .collect();
    mixed_quoter::check(ctx, &legs).await?;
    let pair = (live[0].order.token_in, live[0].order.token_out);
    let plan_legs: Vec<(Entry, Order)> = live.into_iter().map(|l| (l.entry, l.order)).collect();
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let now = q.heads.fresh(ctx)?.timestamp;
    let p = universal::build(ctx, &plan_legs, wallet, now).await?;
    let guard = guard.ok_or_else(|| ctx.err(ErrorCode::Internal, "умный маршрут без долей"))?;
    let d = send_plan(env, &slot, p, guard).await?;
    drop(slot);
    result::result(ctx, req, d, wallet, pair, decs)
}
