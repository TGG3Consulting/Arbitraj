//! Восстанавливает исходную транзакцию после ответа строителя или перезапуска. Сверяет исходный счёт, хеш, блок, calldata и квитанцию.
use super::super::{exec, num, parts::Parts, resolve, send, wallet};
use super::{
    execute::{Marker, identity, same_request},
    missed::missed,
    receipt::fill,
};
use crate::{
    core::types::{Fee, SendMethod, Side, TradeRequest, TradeResult, TradeStatus, TxOutcome},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        pending::PendingRef,
        settlement_intent as intent,
    },
};
use alloy::primitives::{Address, B256, Bytes, U64, U256};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Inclusion {
    from: Address,
    to: Address,
    #[serde(flatten)]
    receipt: send::receipt::Receipt,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    hash: B256,
    parent_hash: B256,
    timestamp: U64,
    number: U64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Original {
    hash: B256,
    block_hash: B256,
    from: Address,
    to: Address,
    nonce: U64,
    input: Bytes,
    value: U256,
}

pub(in crate::venues::bsc_dex) async fn reconcile(
    p: &Parts,
    req: TradeRequest,
    marker: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let ctx = &p.ctx;
    let unknown = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Fermi: исход по сохранённой подписи ещё не подтверждён",
        )
        .pending(marker.clone())
    };
    let parse: Marker =
        serde_json::from_value(marker.parse.clone().ok_or_else(unknown)?).map_err(|_| unknown())?;
    let (hash, nonce) = resolve::keys(p, marker)?;
    let nonce = nonce.ok_or_else(unknown)?;
    let o = &parse.order;
    let db = ctx.journal.db();
    if ctx.chain_id() != Some(1)
        || identity(p, &req)? != parse.key
        || wallet::signer::address(ctx)? != o.payer
    {
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
    if o.prop.is_none()
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
    if o.prop.is_some() {
        send::canonical_receipt::verify(ctx, hash, &r.receipt)
            .await
            .map_err(|_| unknown())?;
    }
    let ok = r.receipt.status == U64::from(1);
    if o.prop.is_some() {
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
        // On a revert, a prior successful transaction could already have consumed this intent.
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
    let (coins, cash) = if req.side == Side::Buy {
        (received, spent)
    } else {
        (spent, received)
    };
    let coins = num::to_decimal(coins, parse.decs.0).ok_or_else(unknown)?;
    let gross = num::to_decimal(cash, parse.decs.1).ok_or_else(unknown)?;
    // A bounded exact-input purchase can return extra coins at the very same approved cash spend.
    // The whole chain fill is credited to the deal. Fermi and every sale still require exact quantity.
    if coins != req.qty && !(o.prop.is_some() && req.side == Side::Buy && coins > req.qty) {
        return Err(unknown());
    }
    let fee = chain.gas_bnb.ok_or_else(unknown)?;
    if let Some(intent::Outcome::Order(out)) = &entry.result {
        if out.trade.filled_qty != coins
            || out.gross_quote != gross
            || out.trade.venue_ref != hash.to_string()
            || out.trade.chain.as_ref().is_none_or(|c| {
                (c.hash, c.nonce, c.block, c.gas_used, c.gas_bnb)
                    != (
                        chain.hash,
                        chain.nonce,
                        chain.block,
                        chain.gas_used,
                        chain.gas_bnb,
                    )
            })
        {
            return Err(unknown());
        }
        resolve::release_if_ours(p, hash, Some(nonce), false).await;
        intent::note_gross(Some(gross));
        return Ok(Some(out.trade.clone()));
    }
    let trade = TradeResult {
        status: TradeStatus::Filled,
        filled_qty: coins,
        avg_price: gross.checked_div(coins).ok_or_else(unknown)?,
        fees: Fee::single(fee, "ETH"),
        venue_ref: hash.to_string(),
        chain: Some(chain),
        finished_at: Utc::now(),
        executor: Some(match &o.prop {
            Some(q) => format!("PropAMM {} ({})", q.terms.venue, o.builder),
            None => format!("FermiSwap ({})", o.builder),
        }),
        request: req,
    };
    let spot = intent::SpotFill {
        trade: trade.clone(),
        gross_quote: gross,
    };
    intent::confirm(
        db,
        &parse.key,
        &entry,
        intent::Outcome::Order(Box::new(spot)),
    )
    .await
    .map_err(|_| unknown())?;
    resolve::release_if_ours(p, hash, Some(nonce), false).await;
    intent::note_gross(Some(gross));
    Ok(Some(trade))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rpc_inclusion_preserves_identity_for_canonical_position_checks() {
        let tx = B256::with_last_byte(1);
        let block = B256::with_last_byte(2);
        let raw = json!({"from":Address::ZERO,"to":Address::ZERO,"transactionHash":tx,"blockHash":block,"transactionIndex":"0x3",
            "blockNumber":"0x5","gasUsed":"0x5208","effectiveGasPrice":"0x1","status":"0x1","logs":[]});
        let r: Inclusion = serde_json::from_value(raw).unwrap();
        assert_eq!(
            (
                r.receipt.transaction_hash,
                r.receipt.block_hash,
                r.receipt.transaction_index
            ),
            (Some(tx), Some(block), Some(U64::from(3)))
        );
    }
}
