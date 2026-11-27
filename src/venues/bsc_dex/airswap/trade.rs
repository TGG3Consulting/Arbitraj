//! Исполнение одной активной котировки с исходного кошелька. Хеш и nonce сохраняются до передачи каналу.
use super::{LABEL, enabled, profile, routing::Ticket, timely};
use crate::{
    core::types::{Side, TradeRequest, TradeResult, TxOutcome},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        events::MarketEvent,
    },
    venues::bsc_dex::{
        exec, hashflow, num, parts::Parts, pools::exact_wallet_tokens, quote::white_state, wallet,
    },
};
use alloy_primitives::U256;
use rust_decimal::Decimal;

pub(in crate::venues::bsc_dex) async fn trade(
    p: &Parts,
    req: TradeRequest,
    t: &Ticket,
) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    ctx.bound()?;
    let reject = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Условия принятой заявки AirSwap не совпадают с торговым поручением",
        )
    };
    let valid =
        || enabled(ctx) && ctx.settings.generation() == t.generation && timely(ctx, t.proof.expiry);
    let who = wallet::signer::address(ctx)?;
    let coin = req.asset.contract.ok_or_else(reject)?;
    let stable = p.tokens.usdt(ctx)?;
    let buy = req.side == Side::Buy;
    let pair = if buy { (stable, coin) } else { (coin, stable) };
    let (asset, cash, cd, sd) = if buy {
        (t.received, t.spent, t.dout, t.di)
    } else {
        (t.spent, t.received, t.di, t.dout)
    };
    let bound = req
        .qty
        .checked_mul(req.worst_price)
        .filter(|n| *n > Decimal::ZERO)
        .ok_or_else(reject)?;
    let money = num::to_decimal(cash, sd).ok_or_else(reject)?;
    if !valid()
        || !super::routing::within_input(t.proof.amount, t.di, if buy { bound } else { req.qty })
        || req.venue != ctx.venue()
        || req.remeasure.as_ref().is_some_and(|r| !r.any_executor)
        || who != t.proof.call.payer
        || pair != (t.proof.input, t.proof.output)
        || num::to_decimal(asset, cd) != Some(req.qty)
        || (if buy {
            money > bound
        } else {
            money < req.remeasure.as_ref().map_or(bound, |r| bound.max(r.floor))
        })
    {
        return Err(reject());
    }
    // Минимум, вошедший в подписываемый calldata, обеспечивает и выставленную цену сделки.
    let required = if buy {
        req.qty
    } else {
        req.remeasure.as_ref().map_or(bound, |r| bound.max(r.floor))
    };
    if num::to_decimal(t.proof.minimum, if buy { cd } else { sd }).is_none_or(|n| n < required) {
        return Err(reject());
    }
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
    if slot.from != who || !valid() {
        return Err(reject());
    }
    let h = white_state::head(ctx, "latest").await?;
    if !profile::Profile::candidates(ctx).contains(&t.proof.profile) {
        return Err(reject());
    }
    let (_, fee) = t.proof.profile.check(ctx, h, who).await?;
    if fee != t.proof.fee {
        return Err(reject());
    }
    if t.proof.profile.module.is_none() {
        exact_wallet_tokens::check(ctx, "airswap", pair.0, pair.1, &h.selector()).await?;
    }
    let (spent, received, _) = t.proof.measure(ctx, h, &t.data).await?;
    if (spent, received) != (t.spent, t.received) || !valid() {
        return Err(reject());
    }
    let decs = (cd, sd);
    let mut parse = exec::swap_ctx::SwapCtx::swap(who, pair, decs, &[], None, None);
    parse.airswap = Some(Box::new(t.proof.clone()));
    parse.who = Some(LABEL.into());
    let raw = exec::txs::send(
        &env,
        &slot,
        t.proof.call.module,
        t.data.clone(),
        U256::ZERO,
        false,
        Some(hold),
    );
    let done = hashflow::native_send(p, &req, t.proof.expiry, t.generation, &parse, raw).await?;
    if matches!(
        done.chain.outcome,
        TxOutcome::Included | TxOutcome::Reverted
    ) {
        let r = done.receipt.as_ref().ok_or_else(|| {
            parse.attach(
                ctx.err(ErrorCode::OutcomeUnknown, "Нет исходной квитанции AirSwap")
                    .with_tx(done.chain.clone()),
            )
        })?;
        t.proof
            .included(ctx, done.chain.hash, done.chain.nonce, r)
            .await
            .map_err(|e| parse.attach(e.with_tx(done.chain.clone())))?;
    }
    exec::result::by(
        exec::result::with_native_fact(ctx, req, done, who, pair, decs, &[], Some(&parse))
            .map_err(|e| parse.attach(e)),
        LABEL,
    )
}
