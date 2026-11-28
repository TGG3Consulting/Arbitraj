//! Подпись остаётся в памяти. Один POST возможен только у владельца записи денежного шага и Permit2 nonce.
use super::{
    Terms, client,
    ledger::{Marker, identity},
    market::{Config, Ticket},
    recovery,
};
use crate::{
    core::types::{SendMethod, Side, TradeRequest, TradeResult},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        settlement_intent as intent,
    },
    venues::bsc_dex::{num, parts::Parts, quote::white_state, send, wallet::signer},
};
use alloy::{primitives::B256, signers::SignerSync};
use chrono::Utc;
use rust_decimal::Decimal;

pub(super) fn channels(p: &Parts) -> bool {
    send::select(&p.ctx, &p.stats)
        .is_ok_and(|c| !c.is_empty() && c.iter().all(|s| s.method == SendMethod::Single))
}
fn fits(r: &TradeRequest, t: &Ticket, terms: &Terms) -> Option<()> {
    if r.qty != t.qty
        || r.side != t.side
        || r.qty <= Decimal::ZERO
        || r.worst_price <= Decimal::ZERO
        || r.remeasure.as_ref().is_some_and(|s| !s.any_executor)
    {
        return None;
    }
    let buy = r.side == Side::Buy;
    let coin = r.asset.contract?;
    if coin != if buy { t.req.output } else { t.req.input } {
        return None;
    }
    let limit = r.qty.checked_mul(r.worst_price)?;
    let (amount, stable) = if buy {
        (terms.min_output, terms.maximum_input)
    } else {
        (
            terms.maximum_input,
            t.cfg.output.guaranteed_net(terms.min_output),
        )
    };
    let qty = num::to_decimal(amount, t.decimals.0)?;
    let cash = num::to_decimal(stable, t.decimals.1)?;
    let floor = r.remeasure.as_ref().map_or(limit, |s| s.floor.max(limit));
    ((if buy {
        qty >= r.qty && cash <= limit
    } else {
        qty == r.qty && cash >= floor
    }) && cash > Decimal::ZERO)
        .then_some(())
}
pub(in crate::venues::bsc_dex) async fn trade(
    p: &Parts,
    r: TradeRequest,
    id: B256,
) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    let db = ctx.journal.db();
    if let Some(done) = recovery::prior(p, &r).await? {
        return Ok(done);
    }
    let bad = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "PCSX: кошелёк, расчётный актив, предел цены или срок заявки изменились",
        )
    };
    let key = identity(ctx, &r).ok_or_else(bad)?;
    let wallet = signer::signer(ctx)?;
    let who = wallet.address();
    let ticket = p.pancakex.take(who, id).ok_or_else(bad)?;
    let t = ticket
        .offer
        .terms(&ticket.cfg.p, &ticket.req)
        .ok_or_else(bad)?;
    fits(&r, &ticket, &t).ok_or_else(bad)?;
    let cash = if r.side == Side::Buy {
        ticket.req.input
    } else {
        ticket.req.output
    };
    if p.tokens.usdt(ctx)? != cash || !channels(p) || ticket.req.owner != who {
        return Err(bad());
    }
    let slot = p.nonces.acquire(ctx).await?;
    let generation = ctx.settings.generation();
    if slot.from != who
        || Config::read(ctx, ticket.req.input, ticket.req.output).as_ref() != Some(&ticket.cfg)
    {
        return Err(bad());
    }
    let (h, fin) = tokio::try_join!(
        white_state::head(ctx, "latest"),
        white_state::head(ctx, "finalized")
    )?;
    ticket.eligible(ctx, h).await.ok_or_else(bad)?;
    let now = || Utc::now().timestamp().max(0) as u64;
    if ctx.settings.generation() != generation || !ticket.timely(now()) {
        return Err(bad());
    }
    // Регистрация в памяти предшествует любому await записи и публикации подписи.
    let writing = db
        .ledger()
        .pancakex_orders
        .begin(ticket.cfg.p.chain, &key)
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::OutcomeUnknown,
                "PCSX: поручение уже регистрируется либо повреждена бронь Permit2",
            )
        })?;
    let signature = wallet.sign_hash_sync(&t.digest).map_err(|_| bad())?;
    let limits = ctx.limits()?;
    let marker = Marker {
        key: key.clone(),
        account: ctx.bound()?.to_string(),
        ticket,
        start: fin.number.to(),
        anchor: fin.hash,
        digest: t.digest,
    };
    let pending = marker.pending();
    let unknown = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "PCSX: исход исходной подписи запрашивается по её хешу",
        )
        .pending(pending.clone())
    };
    intent::scope(
        db,
        &key,
        intent::Request::Order(Box::new(r.clone())),
        intent::before_send(&pending),
    )
    .await?;
    writing.recorded();
    let reserved = marker.reserve(db, &t).await.map_err(|_| unknown())?;
    if reserved && ctx.settings.generation() == generation && marker.ticket.timely(now()) {
        let _reply = client::publish(
            &marker.ticket.cfg.base,
            marker.ticket.cfg.p.chain,
            &marker.ticket.offer,
            &signature,
            limits,
        )
        .await;
    }
    drop(slot);
    recovery::reconcile(p, r, &pending)
        .await?
        .ok_or_else(unknown)
}
