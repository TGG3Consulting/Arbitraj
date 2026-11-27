//! Отправка ордера Fermi или PropAMM по выбранному билету. Вызывается после проверки типа денежного шага.
//! Перед передачей строителю сохраняет хеш и nonce; повтор со старым ключом направляется на восстановление.
use super::{Marker, check, identity, same_request, timely};
use super::super::{dispatch, recovery::reconcile, slot_config};
use crate::{core::types::{Side, TradeRequest, TradeResult}, gateway::{
    errors::{ErrorCode, GatewayResult}, events::MarketEvent, pending::PendingRef, settlement_intent as intent,
}, venues::bsc_dex::{exec, parts::Parts, resolve, send, wallet}};
use alloy::primitives::{Address, B256, TxKind, U256, keccak256};
use chrono::Utc;
use std::time::Duration;

pub(super) async fn submit(
    p: &Parts,
    req: TradeRequest,
    id: B256,
    outer_cash: Option<Address>,
) -> GatewayResult<TradeResult> {
    let ctx = &p.ctx;
    ctx.bound()?;
    let local_key = identity(p, &req)?;
    let key = if outer_cash.is_some() {
        intent::current_key()
            .ok_or_else(|| ctx.err(ErrorCode::NotApplicable, "Отсутствует ключ денежного шага"))?
    } else {
        local_key
    };
    let db = ctx.journal.db();
    let storage = |e: String| ctx.err(ErrorCode::OutcomeUnknown, e);
    let pending = |m: &PendingRef| {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Fermi: сначала требуется исход по сохранённым хешу и nonce",
        )
        .pending(m.clone())
    };
    if let Some(entry) = intent::load(db, &key).await.map_err(storage)? {
        let original = entry
            .lookup
            .parse
            .clone()
            .and_then(|value| serde_json::from_value::<Marker>(value).ok());
        if ctx.chain_id() != Some(1)
            || original.as_ref().is_none_or(|m| {
                m.key != key || Some(m.order.payer) != wallet::signer::address(ctx).ok()
            })
        {
            return Err(pending(&entry.lookup));
        }
        if original.as_ref().is_some_and(|m| m.order.prop.is_some()) {
            return reconcile(p, req, &entry.lookup)
                .await?
                .ok_or_else(|| pending(&entry.lookup));
        }
        if let Some(intent::Outcome::Order(out)) = entry.result
            && same_request(&req, &out.trade.request)
        {
            if let Some(t) = &out.trade.chain {
                resolve::release_if_ours(p, t.hash, Some(t.nonce), false).await;
            }
            intent::note_gross(Some(out.gross_quote));
            return Ok(out.trade);
        }
        return Err(pending(&entry.lookup));
    }
    let signer = wallet::signer::signer(ctx)?;
    let who = signer.address();
    let q = p.slot_markets.tickets.take(who, id).ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            "В памяти нет выбранного билета Fermi этого кошелька",
        )
    })?;
    if outer_cash.is_some_and(|cash| {
        q.proof.prop.is_none()
            || cash != q.book.config.cash
            || cash
                != (if req.side == Side::Buy {
                    q.proof.input
                } else {
                    q.proof.output
                })
    }) {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "Контракт расчётной валюты расходится с билетом строителя",
        ));
    }
    check(p, &req, &q, who)?;
    let notice = |text| {
        p.stats.emit(MarketEvent::Notice {
            venue: ctx.venue(),
            code: "dex.swap_waits_inflow".into(),
            text,
        })
    };
    let _inflows = p.inflows.swap(ctx, q.proof.output, &notice).await?;
    let nonce_slot = p.nonces.acquire(ctx).await?;
    check(p, &req, &q, nonce_slot.from)?;
    let nonce = nonce_slot.nonce(ctx).await?;
    let mut proof = q.proof.clone();
    proof.leg = keccak256(key.as_bytes());
    let (amount, proceeds, gas) =
        slot_config::simulate(ctx, &q.book, &proof, (q.fees.max_fee, q.fees.tip)).await?;
    let required = proof.prop.as_ref().map_or(q.received, |r| r.minimum);
    if amount > q.spent || proceeds < required || gas > q.gas_limit {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "Fermi: повторная симуляция превысила пределы цены или газа",
        ));
    }
    let need = U256::from(q.gas_limit) * U256::from(q.fees.max_fee);
    exec::gas_fund::ensure(ctx, who, need).await?;
    if !slot_config::fresh(p, &q.book).await {
        return Err(ctx.err(ErrorCode::StaleData, "Fermi: кадр закрыт до подписи"));
    }
    let tx = send::tx_price::TxPrice::Eip1559(q.fees, q.base).sign(
        ctx,
        &signer,
        "Подпись Fermi",
        1,
        nonce,
        q.gas_limit,
        (TxKind::Call(proof.executor), U256::ZERO, proof.calldata()),
    )?;
    let parse = Marker {
        order: proof,
        decs: q.decs,
        key: key.clone(),
        outer_account: outer_cash.and(ctx.account.clone()),
    };
    let marker = PendingRef {
        nonce: Some(nonce),
        venue_ref: Some(tx.hash.to_string()),
        parse: Some(
            serde_json::to_value(&parse)
                .map_err(|e| ctx.err(ErrorCode::Internal, e.to_string()))?,
        ),
        ..Default::default()
    };
    if outer_cash.is_some() {
        intent::before_send(&marker).await?;
    } else {
        intent::scope(
            db,
            &key,
            intent::Request::Order(Box::new(req.clone())),
            intent::before_send(&marker),
        )
        .await?;
    }
    exec::settlement_swap::reserve(ctx, &nonce_slot, tx.hash, nonce)
        .await
        .map_err(|_| pending(&marker))?;
    if !timely(p, &q) {
        let entry = intent::load(db, &key)
            .await
            .map_err(|_| pending(&marker))?
            .ok_or_else(|| pending(&marker))?;
        if entry.lookup.venue_ref != marker.venue_ref {
            return Err(pending(&marker));
        }
        let free = nonce_slot.hold().free();
        wallet::queue_store::save(ctx, &free).await;
        intent::archive_attempt(db, &key, &entry, "Слот Fermi закрыт до сетевой отправки")
            .await
            .map_err(|_| pending(&marker))?;

        return Err(ctx.err(ErrorCode::StaleData, "Слот Fermi закрыт перед отправкой"));
    }
    let sent = dispatch::send(
        ctx,
        &q.book.config,
        &signer,
        &tx.raw,
        q.proof.target,
        q.proof.timestamp,
        q.book.frame.deployment.engine,
    )
    .await;
    drop(nonce_slot);
    // В том числе при потерянном ответе ищем только исходную подпись. Вторичного транспорта нет.
    let now = Utc::now().timestamp_millis().max(0) as u64;
    let until_target = q.proof.timestamp.saturating_mul(1000).saturating_sub(now);
    if sent.is_ok() {
        tokio::time::sleep(Duration::from_millis(until_target)).await;
    }
    match reconcile(p, req, &marker).await {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err(pending(&marker)),
        Err(e) => Err(e),
    }
}
