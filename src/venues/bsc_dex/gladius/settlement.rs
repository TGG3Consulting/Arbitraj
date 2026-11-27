//! Исполнение выбранной заявки с повторной проверкой на месте в очереди своего кошелька.
pub(in crate::venues::bsc_dex) use super::simulate::Saved;
use super::{profile::Profile, routing::Ticket, simulate::Payment, *};
use crate::{
    core::types::{Side, TradeRequest, TradeResult, TxOutcome},
    gateway::events::MarketEvent,
    venues::bsc_dex::{exec, hashflow, num, quote::direct, wallet},
};
pub(in crate::venues::bsc_dex) async fn trade(
    p: &Parts,
    req: TradeRequest,
    t: &Ticket,
) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    ctx.bound()?;
    let bad = || {
        no(
            ctx,
            "Gladius: условия, счёт, допуск или срок выбранной заявки изменились",
        )
    };
    let profile = Profile::read(ctx).ok_or_else(bad)?;
    let who = wallet::signer::address(ctx)?;
    let valid = || {
        enabled(ctx)
            && ctx.settings.generation() == t.generation
            && timely(ctx, t.offer.expiry)
            && p.gladius.rows(p).is_some_and(|rows| {
                rows.iter()
                    .any(|r| r.api.order_hash == t.offer.api.order_hash)
            })
    };
    let buy = req.side == Side::Buy;
    let coin = req.asset.contract.ok_or_else(bad)?;
    let quote = p.tokens.usdt(ctx)?;
    let pair = if buy { (quote, coin) } else { (coin, quote) };
    if !valid()
        || who != t.owner
        || pair != t.offer.pair()
        || req.venue != ctx.venue()
        || req.qty != t.qty
        || req.side != t.side
        || req.worst_price <= Decimal::ZERO
        || req.remeasure.as_ref().is_some_and(|v| !v.any_executor)
    {
        return Err(bad());
    }
    let limit = req.qty.checked_mul(req.worst_price).ok_or_else(bad)?;
    let wanted = direct::ceil_units(req.qty, t.decs.0).ok_or_else(bad)?;
    if buy && num::to_decimal(t.spent, t.decs.1).is_none_or(|v| v > limit)
        || !buy && t.spent != wanted
    {
        return Err(bad());
    }
    let floor = if buy {
        wanted
    } else {
        direct::ceil_units(
            req.remeasure.as_ref().map_or(limit, |r| limit.max(r.floor)),
            t.decs.1,
        )
        .ok_or_else(bad)?
    };
    if t.received < floor {
        return Err(bad());
    }
    let payment = Payment::new(
        profile,
        who,
        &t.offer,
        t.quantity,
        if buy { U256::ZERO } else { wanted },
        t.spent,
        floor,
    );
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
        return Err(bad());
    }
    let h = white_state::head(ctx, "latest").await?;
    profile.verify(ctx, h).await?;
    payment.measure(ctx, h, t.offer.expiry).await?;
    if !valid() {
        return Err(bad());
    }
    let mut parse = exec::swap_ctx::SwapCtx::swap(who, pair, t.decs, &[], None, None);
    parse.gladius = Some(Box::new(payment.saved.clone()));
    parse.who = Some(LABEL.into());
    let raw = exec::txs::send(
        &env,
        &slot,
        profile.module.address,
        payment.data,
        U256::ZERO,
        false,
        Some(hold),
    );
    let done = hashflow::native_send(p, &req, t.offer.expiry, t.generation, &parse, raw).await?;
    if matches!(
        done.chain.outcome,
        TxOutcome::Included | TxOutcome::Reverted
    ) {
        let r = done.receipt.as_ref().ok_or_else(|| {
            parse.attach(
                ctx.err(ErrorCode::OutcomeUnknown, "Нет исходной квитанции Gladius")
                    .with_tx(done.chain.clone()),
            )
        })?;
        payment
            .saved
            .included(ctx, done.chain.hash, done.chain.nonce, r)
            .await
            .map_err(|e| parse.attach(e.with_tx(done.chain.clone())))?;
    }
    exec::result::by(
        exec::result::with_native_fact(ctx, req, done, who, pair, t.decs, &[], Some(&parse))
            .map_err(|e| parse.attach(e)),
        LABEL,
    )
}
use rust_decimal::Decimal;
