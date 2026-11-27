//! Устойчивое восстановление из исходного собственного заказа и полной истории сети.
use super::{
    history::{self, Events},
    ledger::{self, Marker},
    result::{empty, paid},
};
use crate::{
    core::types::{TradeRequest, TradeResult},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        pending::PendingRef,
        settlement_intent as intent,
    },
    venues::bsc_dex::{parts::Parts, quote::white_state},
};
pub(super) fn closed(expired: bool, unused: bool, events: &Events) -> bool {
    expired && events.original.is_none() && (unused || events.excludes)
}
pub(in crate::venues::bsc_dex) async fn reconcile(
    p: &Parts,
    r: TradeRequest,
    pend: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let ctx = &p.ctx;
    let db = ctx.journal.db();
    let unknown = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "PCSX: ожидается исход первоначальной подписи Permit2",
        )
        .pending(pend.clone())
    };
    let m = Marker::read(pend).ok_or_else(unknown)?;
    let t = m.terms().ok_or_else(unknown)?;
    if ctx.chain_id() != Some(m.ticket.cfg.p.chain)
        || ctx.account.as_deref() != Some(&m.account)
        || ctx.venue() != r.venue
    {
        return Err(unknown());
    }
    let entry = intent::load(db, &m.key)
        .await
        .map_err(|_| unknown())?
        .ok_or_else(unknown)?;
    if entry.lookup != *pend
        || !matches!(&entry.operation,intent::Request::Order(s) if s.deal_id==r.deal_id&&s.asset==r.asset&&s.venue==r.venue&&s.side==r.side&&s.qty==r.qty)
    {
        return Err(unknown());
    }
    let saved = entry.result.as_ref().and_then(|o| match o {
        intent::Outcome::Order(f) => Some(f),
        _ => None,
    });
    let holder = m.holder(db, &t).await.map_err(|_| unknown())?;
    let own = holder.as_ref() == Some(&m.claim());
    let fresh = if holder.is_some() && !own {
        empty(r, m.digest)
    } else {
        let fin = white_state::head(ctx, "finalized")
            .await
            .map_err(|_| unknown())?;
        let expired = history::expired(fin.timestamp.to(), t.deadline);
        let tip = if expired {
            fin
        } else {
            white_state::head(ctx, "latest")
                .await
                .map_err(|_| unknown())?
        };
        let (cutoff, events) = history::scan(ctx, &m, &t, tip)
            .await
            .map_err(|_| unknown())?;
        if let Some(id) = events.original {
            if !own {
                return Err(unknown());
            }
            paid(p, r, &m, &t, id).await.map_err(|_| unknown())?
        } else {
            if !expired {
                return Ok(None);
            }
            m.ticket
                .cfg
                .p
                .check(ctx, cutoff, &m.ticket.offer.version)
                .await
                .map_err(|_| unknown())?;
            let unused = m
                .ticket
                .cfg
                .p
                .unused(ctx, cutoff, m.ticket.req.owner, t.nonce)
                .await
                .map_err(|_| unknown())?;
            if !closed(expired, unused, &events) {
                return Ok(None);
            }
            empty(r, m.digest)
        }
    };
    if let Some(old) = saved
        && (old.trade.status != fresh.trade.status
            || old.trade.filled_qty != fresh.trade.filled_qty
            || old.gross_quote != fresh.gross_quote)
    {
        return Err(unknown());
    }
    intent::confirm(
        db,
        &m.key,
        &entry,
        intent::Outcome::Order(Box::new(fresh.clone())),
    )
    .await
    .map_err(|_| unknown())?;
    intent::note_gross(Some(fresh.gross_quote));
    Ok(Some(fresh.trade))
}
pub(in crate::venues::bsc_dex) async fn prior(
    p: &Parts,
    r: &TradeRequest,
) -> GatewayResult<Option<TradeResult>> {
    let Some(key) = ledger::identity(&p.ctx, r) else {
        return Ok(None);
    };
    let record = intent::load(p.ctx.journal.db(), &key)
        .await
        .map_err(|e| p.ctx.err(ErrorCode::OutcomeUnknown, e))?;
    let Some(entry) = record.filter(|e| {
        e.lookup
            .parse
            .as_ref()
            .is_some_and(|v| v.get("pancakex").is_some())
    }) else {
        return Ok(None);
    };
    reconcile(p, r.clone(), &entry.lookup)
        .await?
        .map(Some)
        .ok_or_else(|| {
            p.ctx
                .err(
                    ErrorCode::OutcomeUnknown,
                    "PCSX: ожидается исход ранее записанного денежного шага",
                )
                .pending(entry.lookup)
        })
}
