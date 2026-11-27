//! Штатный выбор подписывает только проверенные цены и путь. Перед единственным POST журнал содержит исходные EIP‑712 поля и digest.
use super::super::{num, parts::Parts, quote::white_state, wallet::signer};
use super::{
    deployment::Config,
    http,
    ledger::{self, Marker},
    market::{Snapshot, Ticket},
    recovery,
    wire::SwapIntent,
};
use crate::{
    core::types::{Side, TradeRequest, TradeResult},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        settlement_intent as intent,
    },
};
use alloy::{
    primitives::{B256, U256},
    signers::SignerSync,
};
use chrono::Utc;
use rust_decimal::Decimal;

fn bounds(r: &TradeRequest, old: &Ticket, new: &Ticket, cash: Decimal) -> Option<SwapIntent> {
    let coin = r.asset.contract?;
    let buy = r.side == Side::Buy;
    if r.qty <= Decimal::ZERO
        || r.worst_price <= Decimal::ZERO
        || r.side != old.side
        || r.qty != old.qty
        || (if buy {
            new.order.tokenOut
        } else {
            new.order.tokenIn
        }) != coin
        || new.config != old.config
        || new.order.path != old.order.path
        || new.order.nonce != old.order.nonce
        || new.order.user != old.order.user
        || new.decimals != old.decimals
        || new.order.amountIn > old.order.amountIn
        || r.remeasure.as_ref().is_some_and(|s| !s.any_executor)
    {
        return None;
    }
    let limit = r.qty.checked_mul(r.worst_price)?;
    if (buy && cash > limit) || (!buy && cash < limit) {
        return None;
    }
    let mut o = new.order.clone();
    o.minAmountOut = o.minAmountOut.max(old.order.minAmountOut);
    if !buy {
        let floor = r.remeasure.as_ref().map_or(limit, |s| s.floor.max(limit));
        let mut raw = num::to_units(floor, new.decimals.1)?;
        if num::to_decimal(raw, new.decimals.1)? < floor {
            raw = raw.checked_add(U256::ONE)?;
        }
        o.minAmountOut = o.minAmountOut.max(raw);
        if num::to_decimal(o.minAmountOut, new.decimals.1)? > cash {
            return None;
        }
    }
    Some(o)
}

pub(in crate::venues::bsc_dex) async fn trade(
    p: &Parts,
    r: TradeRequest,
    id: B256,
) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    let db = ctx.journal.db();
    let account = ctx.bound()?.to_string();
    if let Some(t) = recovery::prior(p, &r).await? {
        return Ok(t);
    }
    let key = ledger::identity(ctx, &r)?;
    let bad = || {
        ctx.err(
            ErrorCode::QuoteMismatch,
            "Ryze: изменились счёт, разрешение, путь, размер или допустимый предел",
        )
    };
    let signer = signer::signer(ctx)?;
    let who = signer.address();
    let old = p
        .ryze
        .take(who, id)
        .filter(|t| t.valid(Utc::now()) && Some(&t.config) == Config::read(ctx).as_ref())
        .ok_or_else(bad)?;
    let s = Snapshot::read(p, old.config.clone())
        .await
        .ok_or_else(bad)?;
    let (new, cash) = s
        .measure(p, r.asset.contract.ok_or_else(bad)?, r.side, r.qty)
        .await
        .ok_or_else(bad)?;
    let order = bounds(&r, &old, &new, cash).ok_or_else(bad)?;
    let c = &new.config;
    let url = http::endpoint(&c.relayer, "intents/submit").ok_or_else(bad)?;
    let limits = ctx.limits()?;
    let fin = white_state::head(ctx, "finalized").await?;
    let digest = order.digest(8453, c.router);
    let tokens = c
        .tokens
        .iter()
        .filter(|t| {
            order
                .path
                .iter()
                .any(|h| h.tokenIn == t.address || h.tokenOut == t.address)
        })
        .cloned()
        .collect();
    let m = Marker {
        key: key.clone(),
        account,
        router: c.router,
        runtime: c.router_hash,
        relayer: c.relayer.clone(),
        digest,
        tokens,
        order,
        start: fin.number.to(),
        start_hash: fin.hash,
        decimals: new.decimals,
    };
    let pending = m.pending();
    let unknown = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Ryze: восстанавливается исход первоначального поручения",
        )
        .pending(pending.clone())
    };
    // Mutex общего кошелька охватывает смену цены, подпись и первый HTTP обмен; protocol nonce хранится отдельно от nonce его EOA.
    let slot = p.nonces.acquire(ctx).await?;
    if slot.from != who || m.holder(db).await.map_err(|_| bad())?.is_some() {
        return Err(bad());
    }
    if !new.valid(Utc::now()) || Config::read(ctx).as_ref() != Some(c) {
        return Err(bad());
    }
    let h = white_state::head(ctx, "latest").await?;
    let at = h.selector();
    let nonce = super::super::node::call::canonical(
        ctx,
        &at,
        c.router,
        super::wire::swapNoncesCall { user: who },
    )
    .await?;
    if nonce != m.order.nonce || !super::market::physical(ctx, c, &at, &m.order.path, who).await {
        return Err(bad());
    }
    let sig = signer.sign_hash_sync(&digest).map_err(|_| bad())?;
    let body = m.order.submission(c.router, &sig).ok_or_else(bad)?;
    intent::scope(
        db,
        &key,
        intent::Request::Order(Box::new(r.clone())),
        intent::before_send(&pending),
    )
    .await?;
    if !m.reserve(db).await.map_err(|_| unknown())? {
        // Эта копия выиграла запись денежного шага, но проиграла другой сделке бронь кошелька. Подпись ещё локальна.
        let e = intent::load(db, &key)
            .await
            .map_err(|_| unknown())?
            .ok_or_else(unknown)?;
        if e.lookup == pending {
            intent::confirm(
                db,
                &key,
                &e,
                intent::Outcome::Order(Box::new(recovery::empty(r.clone(), digest))),
            )
            .await
            .map_err(|_| unknown())?;
        }
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Ryze: очередь кошелька уже занята другой сделкой",
        ));
    }
    if new.valid(Utc::now()) && Config::read(ctx).as_ref() == Some(c) {
        let _accepted = http::submit_once(url, digest, body, limits).await;
    }
    drop(slot);
    recovery::reconcile(p, r, &pending)
        .await?
        .ok_or_else(unknown)
}
