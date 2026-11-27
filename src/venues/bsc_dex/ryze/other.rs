//! Другой подписанный intent занял nonce владельца. Проверяем EIP-712, канонический batch и ровно одно событие принятия.
use super::super::{
    ctx::Ctx,
    node::{call::canonical, rpc},
    quote::white_state::{self, Head},
    send,
};
use super::{
    chain,
    ledger::Marker,
    wire::{IntentSwapExecuted, executeSwapIntentsCall, swapNoncesCall},
};
use crate::gateway::errors::GatewayResult;
use alloy::{
    primitives::{Address, B256, Bytes, U256, keccak256},
    sol_types::{SolCall, SolEvent},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Original {
    hash: B256,
    to: Address,
    block_hash: B256,
    input: Bytes,
}

async fn conflicting(ctx: &Ctx, m: &Marker, id: B256, finalized: Head) -> GatewayResult<bool> {
    let (tx, r): (Option<Original>, Option<send::receipt::Receipt>) = tokio::try_join!(
        rpc::call(ctx, "eth_getTransactionByHash", json!([id])),
        rpc::call(ctx, "eth_getTransactionReceipt", json!([id]))
    )?;
    let (Some(tx), Some(r)) = (tx, r) else {
        return Ok(false);
    };
    if tx.hash != id
        || tx.to != m.router
        || r.status.to::<u64>() != 1
        || r.block_number > finalized.number
        || r.transaction_hash != Some(id)
        || r.block_hash != Some(tx.block_hash)
    {
        return Ok(false);
    }
    let Ok(c) = executeSwapIntentsCall::abi_decode_validate(&tx.input) else {
        return Ok(false);
    };
    let mut signed = c
        .signedIntents
        .iter()
        .filter(|s| s.intent.user == m.order.user && s.intent.nonce == m.order.nonce);
    let Some(other) = signed.next().map(|s| &s.intent) else {
        return Ok(false);
    };
    if signed.next().is_some()
        || other.digest(super::CHAIN_ID, m.router) == m.digest
        || !other.present_in(m.router, &tx.input)
    {
        return Ok(false);
    }
    let mut events = r
        .logs
        .iter()
        .filter(|l| {
            l.address() == m.router
                && l.topics().first() == Some(&IntentSwapExecuted::SIGNATURE_HASH)
        })
        .filter_map(|l| IntentSwapExecuted::decode_log_data(&l.inner.data).ok())
        .filter(|e| e.user == other.user && e.nonce == other.nonce);
    let Some(e) = events.next() else {
        return Ok(false);
    };
    if events.next().is_some()
        || (e.tokenIn, e.tokenOut, e.recipient, e.amountIn)
            != (
                other.tokenIn,
                other.tokenOut,
                other.recipient,
                other.amountIn,
            )
        || e.amountOut < other.minAmountOut
    {
        return Ok(false);
    }
    let h = white_state::head(ctx, &format!("{:#x}", r.block_number)).await?;
    if h.hash != tx.block_hash || U256::from(h.timestamp) > other.deadline {
        return Ok(false);
    }
    let code: Bytes = rpc::call(ctx, "eth_getCode", json!([m.router, h.selector()])).await?;
    if keccak256(code) != m.runtime {
        return Ok(false);
    }
    send::canonical_receipt::verify(ctx, id, &r).await?;
    Ok(true)
}

pub(super) async fn consumed(ctx: &Ctx, m: &Marker) -> GatewayResult<bool> {
    let fin = white_state::head(ctx, "finalized").await?;
    if U256::from(fin.timestamp) <= m.order.deadline {
        return Ok(false);
    }
    let end = chain::last_valid(ctx, m, fin).await?.saturating_add(1);
    let after = white_state::head(ctx, &format!("{end:#x}")).await?;
    if end > fin.number.to::<u64>() || U256::from(after.timestamp) <= m.order.deadline {
        return Ok(false);
    }
    let at = after.selector();
    let nonce = canonical(ctx, &at, m.router, swapNoncesCall { user: m.order.user }).await?;
    if nonce <= m.order.nonce {
        return Ok(false);
    }
    let mut start = m.start;
    let mut confirmed = false;
    while start <= end {
        let last = start.saturating_add(999).min(end);
        let rows:Vec<alloy::rpc::types::Log>=rpc::call(ctx,"eth_getLogs",json!([{"address":m.router,
            "topics":[IntentSwapExecuted::SIGNATURE_HASH,m.order.user.into_word()],"fromBlock":format!("{start:#x}"),"toBlock":format!("{last:#x}")}])).await?;
        for row in rows {
            if row.removed
                || row.address() != m.router
                || row.block_number.is_none_or(|v| v < start || v > last)
            {
                return Ok(false);
            }
            let Ok(event) = IntentSwapExecuted::decode_log_data(&row.inner.data) else {
                return Ok(false);
            };
            if event.user != m.order.user || event.nonce != m.order.nonce {
                continue;
            }
            let Some(hash) = row.transaction_hash else {
                return Ok(false);
            };
            if confirmed || !conflicting(ctx, m, hash, fin).await? {
                return Ok(false);
            }
            confirmed = true;
        }
        if last == end {
            break;
        }
        start = last + 1;
    }
    Ok(confirmed)
}
