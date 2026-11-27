//! Устойчивое поручение Fermi. Подпись относится к одному плательщику и слоту; восстановление читает только сохранённый хеш.
use super::super::{exec, num, parts::Parts, resolve, send, wallet};
use super::{dispatch, recovery::reconcile, routing::Prepared, slot::Proof};
use crate::{
    core::types::{Side, TradeRequest, TradeResult},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        events::MarketEvent,
        pending::PendingRef,
        settlement_intent as intent,
    },
};
use alloy::primitives::{Address, B256, TxKind, U256, keccak256};
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

#[derive(Serialize, Deserialize)]
pub(super) struct Marker {
    pub order: Proof,
    pub decs: (u8, u8),
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outer_account: Option<String>,
}
pub(super) fn identity(p: &Parts, r: &TradeRequest) -> GatewayResult<String> {
    if r.deal_id.is_empty() || r.asset.contract.is_none() || r.venue != p.ctx.venue() {
        return Err(p.ctx.err(
            ErrorCode::NotApplicable,
            "Fermi: необходимы исходные ID сделки, монеты и площадки",
        ));
    }
    let account = p.ctx.account.as_deref().ok_or_else(|| {
        p.ctx
            .err(ErrorCode::NotApplicable, "Fermi: требуется счёт сделки")
    })?;
    Ok(format!(
        "fermi/{}",
        keccak256(
            json!([
                1,
                p.ctx.place.id,
                account,
                r.asset.contract,
                r.side,
                r.deal_id
            ])
            .to_string()
            .as_bytes()
        )
    ))
}
pub(super) fn same_request(r: &TradeRequest, t: &TradeRequest) -> bool {
    r.deal_id == t.deal_id
        && r.venue == t.venue
        && r.asset.contract == t.asset.contract
        && r.side == t.side
        && r.qty == t.qty
        && r.asset.chain == t.asset.chain
}
fn timely(p: &Parts, q: &Prepared) -> bool {
    super::slot_config::timely(p, &q.book)
}
fn check(p: &Parts, r: &TradeRequest, q: &Prepared, who: Address) -> GatewayResult<()> {
    let bad = || {
        p.ctx.err(
            ErrorCode::QuoteMismatch,
            "Fermi: сумма, предел цены, счёт или кадр изменились",
        )
    };
    let buy = r.side == Side::Buy;
    let o = &q.proof;
    let coin = r.asset.contract.ok_or_else(bad)?;
    let cash = q.book.config.cash;
    let (i, out) = if buy { (cash, coin) } else { (coin, cash) };
    let fixed = num::to_decimal(o.amount.unsigned_abs(), q.decs.0);
    let gross =
        num::to_decimal(if buy { q.spent } else { q.received }, q.decs.1).ok_or_else(bad)?;
    let limit = r.qty.checked_mul(r.worst_price).ok_or_else(bad)?;
    if !timely(p, q)
        || who != o.payer
        || (i, out) != (o.input, o.output)
        || fixed != Some(r.qty)
        || r.qty <= Decimal::ZERO
        || buy != o.amount.is_negative()
        || (buy && gross > limit)
        || (!buy && gross < limit)
        || r.remeasure
            .as_ref()
            .is_some_and(|v| !v.any_executor || (!buy && gross < v.floor))
    {
        return Err(bad());
    }
    Ok(())
}
pub(in crate::venues::bsc_dex) async fn trade(
    p: &Parts,
    req: TradeRequest,
    id: B256,
) -> GatewayResult<TradeResult> {
    if req
        .route
        .as_ref()
        .is_some_and(crate::gateway::quote_stage::prop_settlement_slot)
    {
        return Err(p.ctx.err(
            ErrorCode::NotApplicable,
            "Для слота нужен исходный денежный шаг",
        ));
    }
    submit(p, req, id, None).await
}
pub(in crate::venues::bsc_dex) async fn trade_settlement(
    p: &Parts,
    req: TradeRequest,
    id: B256,
    cash: Address,
) -> GatewayResult<TradeResult> {
    if !req
        .route
        .as_ref()
        .is_some_and(crate::gateway::quote_stage::prop_settlement_slot)
        || cash != p.tokens.usdt(&p.ctx)?
        || !intent::active()
        || !crate::gateway::settlement_proofs::active()
    {
        return Err(p.ctx.err(
            ErrorCode::NotApplicable,
            "Нет исходного денежного поручения и привязки к его кассе",
        ));
    }
    submit(p, req, id, Some(cash)).await
}
async fn submit(
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
        super::slot_config::simulate(ctx, &q.book, &proof, (q.fees.max_fee, q.fees.tip)).await?;
    let required = proof.prop.as_ref().map_or(q.received, |r| r.minimum);
    if amount > q.spent || proceeds < required || gas > q.gas_limit {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "Fermi: повторная симуляция превысила пределы цены или газа",
        ));
    }
    let need = U256::from(q.gas_limit) * U256::from(q.fees.max_fee);
    exec::gas_fund::ensure(ctx, who, need).await?;
    if !super::slot_config::fresh(p, &q.book).await {
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
