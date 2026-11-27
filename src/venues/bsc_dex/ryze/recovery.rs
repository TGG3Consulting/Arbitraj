//! Повтор денежного шага работает с исходным digest и его квитанцией, включая потерю ответа и перезапуск процесса.
use super::super::{parts::Parts, quote::white_state};
use super::result::{empty, filled};
use super::{
    chain, http,
    ledger::{self, Marker},
};
use crate::{
    core::types::{TradeRequest, TradeResult, TradeStatus},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        pending::PendingRef,
        settlement_intent as intent,
    },
};

fn same(r: &TradeRequest, s: &TradeRequest) -> bool {
    s.deal_id == r.deal_id
        && s.venue == r.venue
        && s.asset == r.asset
        && s.side == r.side
        && s.qty == r.qty
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
            "Ryze: требуется исход сохранённой подписи",
        )
        .pending(pend.clone())
    };
    let m = Marker::read(pend).ok_or_else(unknown)?;
    if ctx.chain_id() != Some(8453)
        || r.venue != ctx.venue()
        || ctx.account.as_deref() != Some(m.account.as_str())
    {
        return Err(unknown());
    }
    let entry = intent::load(db, &m.key)
        .await
        .map_err(|_| unknown())?
        .ok_or_else(unknown)?;
    if entry.lookup != *pend || !matches!(&entry.operation,intent::Request::Order(s) if same(s,&r))
    {
        return Err(unknown());
    }
    let saved = entry.result.as_ref().and_then(|o| match o {
        intent::Outcome::Order(f) => Some(f),
        _ => None,
    });
    let finished = |f: &intent::SpotFill| {
        intent::note_gross(Some(f.gross_quote));
        Some(f.trade.clone())
    };
    if let Some(f) = saved
        && f.trade.status == TradeStatus::NotFilled
    {
        m.release(db).await.map_err(|_| unknown())?;
        return Ok(finished(f));
    }
    let owned = m.holder(db).await.map_err(|_| unknown())? == Some(m.claim());
    let mut found = None;
    if owned || saved.is_some() {
        if let Some(f) = saved {
            let old = f.trade.venue_ref.parse().map_err(|_| unknown())?;
            found = chain::lookup(ctx, &m, old).await.map_err(|_| unknown())?;
        }
        if found.is_none() {
            let url =
                http::endpoint(&m.relayer, &format!("intents/{}", m.digest)).ok_or_else(unknown)?;
            if let Some(id) = http::transaction(url, m.digest, ctx.limits()?).await {
                found = chain::lookup(ctx, &m, id).await.map_err(|_| unknown())?;
            }
        }
        if found.is_none() {
            let tip = white_state::head(ctx, "latest")
                .await
                .map_err(|_| unknown())?;
            for id in chain::hashes(ctx, &m, tip).await.map_err(|_| unknown())? {
                if let Some(t) = chain::lookup(ctx, &m, id).await.map_err(|_| unknown())?
                    && found.replace(t).is_some()
                {
                    return Err(unknown());
                }
            }
        }
    }
    let result = if let Some(t) = found {
        if !m.save_fill(db, &t).await.map_err(|_| unknown())? {
            return Err(unknown());
        }
        let fresh = filled(p, r, &m, t).map_err(|_| unknown())?;
        if let Some(f) = saved {
            if fresh.trade.filled_qty != f.trade.filled_qty || fresh.gross_quote != f.gross_quote {
                return Err(ctx.err(ErrorCode::OutcomeUnknown,"Ryze: та же подпись включена с другой суммой. Обе квитанции сохранены для денежной сверки").pending(pend.clone()));
            }
            let where_at = |f: &intent::SpotFill| {
                f.trade
                    .chain
                    .as_ref()
                    .map(|c| (c.hash, c.block, c.gas_used))
            };
            if where_at(f) == where_at(&fresh) {
                m.release(db).await.map_err(|_| unknown())?;
                return Ok(finished(f));
            }
        }
        fresh
    } else if saved.is_some() {
        return Err(unknown());
    } else if (!owned
        && !m.was_reserved(db).await.map_err(|_| unknown())?
        && alloy::primitives::U256::from(
            white_state::head(ctx, "finalized")
                .await
                .map_err(|_| unknown())?
                .timestamp,
        ) > m.order.deadline)
        || chain::unused_after_deadline(ctx, &m)
            .await
            .map_err(|_| unknown())?
        || super::other::consumed(ctx, &m)
            .await
            .map_err(|_| unknown())?
    {
        empty(r, m.digest)
    } else {
        return Ok(None);
    };
    intent::confirm(
        db,
        &m.key,
        &entry,
        intent::Outcome::Order(Box::new(result.clone())),
    )
    .await
    .map_err(|_| unknown())?;
    m.release(db).await.map_err(|_| unknown())?;
    Ok(finished(&result))
}

pub(in crate::venues::bsc_dex) async fn prior(
    p: &Parts,
    r: &TradeRequest,
) -> GatewayResult<Option<TradeResult>> {
    if p.ctx.chain_id() != Some(8453) || r.deal_id.is_empty() || r.asset.contract.is_none() {
        return Ok(None);
    }
    let key = ledger::identity(&p.ctx, r)?;
    let lookup = intent::load(p.ctx.journal.db(), &key)
        .await
        .map_err(|e| p.ctx.err(ErrorCode::OutcomeUnknown, e))?;
    let Some(e) = lookup.filter(|e| Marker::read(&e.lookup).is_some()) else {
        return Ok(None);
    };
    reconcile(p, r.clone(), &e.lookup)
        .await?
        .map(Some)
        .ok_or_else(|| {
            p.ctx
                .err(
                    ErrorCode::OutcomeUnknown,
                    "Ryze: новое исполнение ждёт исхода сохранённого поручения",
                )
                .pending(e.lookup)
        })
}
