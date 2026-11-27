//! Бронирование nonce и повтор точного подписываемого поручения со средствами исходного EOA.
use super::{routing::Ticket, *};
use crate::{
    core::types::{Side, TradeRequest, TradeResult, TxOutcome},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        events::MarketEvent,
    },
    venues::bsc_dex::{exec, num, quote::direct, wallet},
};
use alloy_primitives::U256;
pub(in crate::venues::bsc_dex) async fn trade(
    p: &Parts,
    req: TradeRequest,
    t: &Ticket,
) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    ctx.bound()?;
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Пара, лимит или допуск Epsilon изменились",
        )
    };
    let who = wallet::signer::address(ctx)?;
    let cash = p.tokens.usdt(ctx)?;
    let coin = req.asset.contract.ok_or_else(bad)?;
    let buy = req.side == Side::Buy;
    let pair = if buy { (cash, coin) } else { (coin, cash) };
    let mut order = t.proof.plan.clone();
    let live = || {
        enabled(ctx)
            && ctx.settings.generation() == t.generation
            && chrono::Utc::now().timestamp() < i64::try_from(order.deadline).unwrap_or(i64::MAX)
    };
    if !live()
        || who != t.proof.call.payer
        || (order.input, order.output) != pair
        || req.qty != t.qty
        || req.side != t.side
        || req.venue != ctx.venue()
        || req.remeasure.as_ref().is_some_and(|x| !x.any_executor)
    {
        return Err(bad());
    }
    let bound = req
        .qty
        .checked_mul(req.worst_price)
        .filter(|x| *x > rust_decimal::Decimal::ZERO)
        .ok_or_else(bad)?;
    let raw = direct::ceil_units(req.qty, t.decs.0).ok_or_else(bad)?;
    if buy {
        if order.minimum < raw || num::to_decimal(order.amount, t.decs.1).is_none_or(|v| v > bound)
        {
            return Err(bad());
        }
    } else {
        if order.amount != raw {
            return Err(bad());
        }
        let min = direct::ceil_units(
            req.remeasure.as_ref().map_or(bound, |r| r.floor.max(bound)),
            t.decs.1,
        )
        .ok_or_else(bad)?;
        order.minimum = order.minimum.max(min);
    }
    let (saved, data) = order.signed(t.proof.call.chain, who).ok_or_else(bad)?;
    let env = p.env();
    let hold = env
        .inflows
        .swap(ctx, pair.1, &|text| {
            env.stats.emit(MarketEvent::Notice {
                venue: ctx.venue(),
                code: "dex.swap_waits_inflow".into(),
                text,
            })
        })
        .await?;
    let slot = p.nonces.acquire(ctx).await?;
    if !live() || slot.from != who {
        return Err(bad());
    }
    let h = white_state::head(ctx, "latest").await?;
    let valid = if let Some(cfg) = &t.config {
        let state = cfg.live(ctx, h).await.ok_or_else(bad)?;
        state.accepts(ctx, h, &order, t.families).await
    } else {
        super::ocean_role::verified(ctx, h, &order).await
    };
    if !valid {
        return Err(bad());
    }
    saved.measure(ctx, h, &data).await?;
    if !live() {
        return Err(bad());
    }
    let mut parse = exec::swap_ctx::SwapCtx::swap(who, pair, t.decs, &[], None, None);
    parse.epsilon = Some(Box::new(saved.clone()));
    let label = if order.ocean.as_ref().is_some_and(|p| !p.nested) {
        DIRECT
    } else {
        LABEL
    };
    parse.who = Some(label.into());
    let raw = exec::txs::send(
        &env,
        &slot,
        saved.call.module,
        data,
        U256::ZERO,
        false,
        Some(hold),
    );
    let done = hashflow::native_send(p, &req, order.deadline, t.generation, &parse, raw).await?;
    if matches!(
        done.chain.outcome,
        TxOutcome::Included | TxOutcome::Reverted
    ) {
        let r = done.receipt.as_ref().ok_or_else(|| {
            parse.attach(
                ctx.err(ErrorCode::OutcomeUnknown, "Нет исходной квитанции Epsilon")
                    .with_tx(done.chain.clone()),
            )
        })?;
        saved
            .included(ctx, done.chain.hash, done.chain.nonce, r)
            .await
            .map_err(|e| parse.attach(e.with_tx(done.chain.clone())))?;
    }
    exec::result::by(
        exec::result::with_native_fact(ctx, req, done, who, pair, t.decs, &[], Some(&parse))
            .map_err(|e| parse.attach(e)),
        label,
    )
}
