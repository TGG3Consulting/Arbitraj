//! Файл: venues/bsc_dex/exec/trade_smart.rs
//! Что делает: сделка по умному маршруту из котировки (решение 38): пересчёт каждой доли по живому состоянию её пулов
//! (один пул или пара через промежуточную монету, решение 52 — exec/live.rs; перед этим — повторная проба крючка B/C,
//! решение 32), проверка средней цены против худшей допустимой, шаги и минимумы каждой доли (exec/chain.rs),
//! ожидаемые поступления выходного токена (wallet/inflows.rs), сверка долей с MixedQuoter PancakeSwap, разрешения
//! и один execute Universal Router PancakeSwap со всеми долями и итоговой проверкой баланса (exec/universal.rs),
//! ожидание итога своим узлом, факт по логам Transfer. Повторов нет (стартовое правило Тиграна).
//! Кто вызывает: exec/trade.rs.
//! От чего зависит: exec/live.rs, exec/chain.rs, exec/universal.rs, exec/trade.rs (общие шаги), exec/result.rs,
//! quote/direct.rs, quote/via.rs, quote/mixed_quoter.rs, wallet/nonce.rs.
//! Настройки: через вызываемые — smart.*, exec.dex_slippage_pct, dex.*, send.*, channel.*.
//!
//! Количество доли: продажа — qty × доля входа (RouteLeg.share); покупка — qty × ожидаемые монеты доли / все
//! ожидаемые монеты; последняя доля получает остаток, чтобы сумма долей была ровно qty.

use rust_decimal::Decimal;

use crate::core::types::{DexRoute, Side, TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;

use super::super::quote::mixed_quoter::{self, Leg};
use super::super::quote::path::Path;
use super::super::quote::via::Via;
use super::super::quote::{Quoting, direct};
use super::super::wallet::nonce::Nonces;
use super::trade::{check_price, send_plan, sides, worst_positive};
use super::txs::Env;
use super::{chain, live, result, universal};

/// Сделка по умному маршруту. Ошибки: NoRoute (доля не годится, пул пропал или не допущен), QuoteMismatch
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
    let (coin, usdt, decs) = sides(q, &req).await?;
    let out_token = if req.side == Side::Buy { coin } else { usdt };
    let notice = |text: String| {
        env.stats.emit(MarketEvent::Notice {
            venue: ctx.venue(),
            code: "dex.swap_waits_inflow".into(),
            text,
        });
    };
    let guard = env.inflows.swap(ctx, out_token, &notice).await?;
    let v = Via {
        q,
        coin,
        usdt,
        side: req.side,
        taxes: &taxes,
        coin_dec: decs.0,
        usdt_dec: decs.1,
    };
    let mut paths: Vec<(Path, Decimal)> = Vec::with_capacity(route.legs.len());
    let (mut coins, mut paid, mut left) = (Decimal::ZERO, Decimal::ZERO, req.qty);
    for (i, (leg, w)) in route.legs.iter().zip(&weights).enumerate() {
        let qty = if i + 1 == route.legs.len() {
            left
        } else {
            (req.qty * w / total).min(left)
        };
        left -= qty;
        let p = live::path(&v, &leg.hops, qty, false).await?;
        coins += p.coins;
        paid += p.usdt;
        paths.push((p, qty));
    }
    check_price(ctx, &req, coins, paid)?;
    let legs: Vec<Leg<'_>> = paths.iter().map(|(p, _)| Leg { path: p }).collect();
    mixed_quoter::check(ctx, &legs).await?;
    let mut steps = Vec::with_capacity(paths.len());
    for (p, qty) in &paths {
        steps.push(chain::steps(ctx, &req, p, *qty, decs)?);
    }
    let slot = nonces.acquire(ctx).await?;
    let wallet = slot.from;
    let now = q.heads.fresh(ctx)?.timestamp;
    let plan = universal::build(ctx, &steps, wallet, now).await?;
    let d = send_plan(env, &slot, plan, guard).await?;
    drop(slot);
    let tin = if req.side == Side::Buy { usdt } else { coin };
    result::result(ctx, req, d, wallet, (tin, out_token), decs)
}
