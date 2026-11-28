//! Восстанавливает исходную транзакцию после ответа строителя или перезапуска. Сверяет исходный счёт, хеш, блок, calldata и квитанцию.
//! На уже включённом обмене передаёт проверенные денежные события в учёт сделки.
#[path = "recovery_order.rs"]
mod order;
#[cfg(test)]
pub(super) use order::{amounts, spot};
#[path = "recovery_origin.rs"]
mod origin;
use origin::{Header, Inclusion, Original, outer_matches, unknown};
use super::super::{exec, parts::Parts, resolve, send, wallet};
use super::{
    execute::{Marker, identity, same_request},
    missed::missed,
    receipt::fill,
};
use crate::{
    core::types::{SendMethod, TradeRequest, TradeResult, TxOutcome},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        pending::PendingRef,
        settlement_intent as intent,
    },
};
use alloy::primitives::U64;
use serde_json::json;
pub(in crate::venues::bsc_dex) async fn reconcile(
    p: &Parts,
    req: TradeRequest,
    marker: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let ctx = &p.ctx;
    let unknown = || unknown(p, marker);
    let parse: Marker =
        serde_json::from_value(marker.parse.clone().ok_or_else(unknown)?).map_err(|_| unknown())?;
    let (hash, nonce) = resolve::keys(p, marker)?;
    let nonce = nonce.ok_or_else(unknown)?;
    let o = &parse.order;
    let db = ctx.journal.db();
    let key_matches = match parse.outer_account.as_deref() {
        Some(account) => {
            o.observed()
                && req.venue == ctx.venue()
                && outer_matches(
                    account,
                    ctx.account.as_deref(),
                    &parse.key,
                    intent::current_key().as_deref(),
                    o.leg,
                )
        }
        None => identity(p, &req)? == parse.key,
    };
    if ctx.chain_id() != Some(1) || !key_matches || wallet::signer::address(ctx)? != o.payer {
        return Err(unknown());
    }
    let entry = intent::load(db, &parse.key)
        .await
        .map_err(|_| unknown())?
        .ok_or_else(unknown)?;
    if entry.lookup.parse != marker.parse
        || entry.lookup.venue_ref != marker.venue_ref
        || entry.lookup.nonce != Some(nonce)
        || !matches!(&entry.operation,intent::Request::Order(r) if same_request(r,&req))
    {
        return Err(unknown());
    }
    if !o.observed()
        && let Some(intent::Outcome::Order(out)) = &entry.result
    {
        resolve::release_if_ours(p, hash, Some(nonce), false).await;
        intent::note_gross(Some(out.gross_quote));
        return Ok(Some(out.trade.clone()));
    }
    let (network, r): (U64, Option<Inclusion>) = tokio::try_join!(
        super::rpc::call(ctx, "eth_chainId", json!([])),
        super::rpc::call(ctx, "eth_getTransactionReceipt", json!([hash]))
    )?;
    if network != U64::from(1) {
        return Err(unknown());
    }
    let Some(r) = r else {
        if entry.result.is_some() {
            return Err(unknown());
        }
        return missed(p, &parse, &entry, hash, nonce, marker).await;
    };
    let (head, tx): (Header, Option<Original>) = tokio::try_join!(
        super::rpc::call(
            ctx,
            "eth_getBlockByNumber",
            json!([r.receipt.block_number, false])
        ),
        super::rpc::call(ctx, "eth_getTransactionByHash", json!([hash]))
    )?;
    let t = tx.ok_or_else(unknown)?;
    let block = r.receipt.block_hash.ok_or_else(unknown)?;
    if (head.hash, t.hash, t.block_hash, r.receipt.transaction_hash)
        != (block, hash, block, Some(hash))
        || head.number != r.receipt.block_number
        || t.nonce != U64::from(nonce)
        || (t.from, t.to) != (o.payer, o.executor)
        || (r.from, r.to) != (t.from, t.to)
        || !t.value.is_zero()
        || t.input != o.calldata()
    {
        return Err(unknown());
    }
    if let Some(order) = o.bop {
        order.verify_at(ctx,block,o.executor).await.map_err(|_|unknown())?;
    }
    let ok = r.receipt.status == U64::from(1);
    if o.observed() {
        if ok && parse.outer_account.is_some() && crate::gateway::settlement_proofs::active() {
            send::canonical_receipt::observe(ctx, hash, &r.receipt)
                .await
                .map_err(|_| unknown())?;
        } else {
            send::canonical_receipt::verify(ctx, hash, &r.receipt)
                .await
                .map_err(|_| unknown())?;
        }
    }
    if o.observed() {
        let filled = super::canonical(
            ctx,
            block,
            o.executor,
            super::slot::completedCall {
                payer: o.payer,
                leg: o.leg,
            },
        )
        .await
        .map_err(|_| unknown())?;
        // Если транзакция откатилась, то поручение могла ранее исполнить другая транзакция.
        if filled != ok {
            return Err(unknown());
        }
    }
    if !ok && entry.result.is_some() {
        return Err(unknown());
    }
    let outcome = if ok {
        TxOutcome::Included
    } else {
        TxOutcome::Reverted
    };
    let tracked = send::track::Tracked {
        outcome,
        receipt: Some(r.receipt.clone()),
        cancel: None,
        unresolved: None,
    };
    let chain = exec::chain_tx::chain_tx(
        hash,
        nonce,
        vec![o.builder.clone()],
        SendMethod::Bundle,
        &tracked,
        None,
    );
    if !ok {
        resolve::release_if_ours(p, hash, Some(nonce), false).await;
        intent::archive_attempt(
            db,
            &parse.key,
            &entry,
            "Исходная транзакция Fermi откатилась в каноническом блоке",
        )
        .await
        .map_err(|_| unknown())?;

        return Err(ctx
            .err(ErrorCode::SwapReverted, "Обмен Fermi откатился")
            .with_tx(chain));
    }
    if head.parent_hash != o.parent
        || head.number.to::<u64>() != o.target
        || head.timestamp.to::<u64>() != o.timestamp
    {
        return Err(unknown());
    }
    // Старые поручения хранили только подписанный адрес исполнителя. Его getter читается
    // в том же каноническом блоке, что и квитанция, независимо от текущего профиля.
    let router = match o.router {
        Some(address) => address,
        None => super::deployment::executor_router(ctx, block, o.executor)
            .await
            .map_err(|_| unknown())?,
    };
    if router.is_zero() {
        return Err(unknown());
    }
    if let Some(q) = &o.prop {
        q.verify_roles_at_fill(ctx, hash, &r.receipt)
            .await
            .map_err(|_| unknown())?;
    }
    let (spent, received) = fill(o, &r.receipt, router).ok_or_else(unknown)?;
    order::finish(p, req, marker, &parse, &entry, chain, (spent, received)).await
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
