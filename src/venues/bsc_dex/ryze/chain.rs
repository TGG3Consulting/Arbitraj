//! Канонический исход Ryze по transaction + receipt. HTTP-ответ, отсутствие receipt и рост nonce отдельно денег не подтверждают.
use super::super::{
    ctx::Ctx,
    node::{call::canonical, rpc},
    quote::white_state::{self, Head},
    send,
};
use super::{
    fill,
    ledger::Marker,
    wire::{IntentSwapExecuted, swapNoncesCall},
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{
    primitives::{Address, B256, Bytes, U64, U256, keccak256},
    sol_types::SolEvent,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Transaction {
    hash: B256,
    block_hash: B256,
    from: Address,
    to: Address,
    input: Bytes,
    nonce: U64,
}
pub struct Mined {
    pub hash: B256,
    pub tx_nonce: u64,
    pub fill: fill::Fill,
    pub receipt: send::receipt::Receipt,
}

pub async fn lookup(ctx: &Ctx, m: &Marker, id: B256) -> GatewayResult<Option<Mined>> {
    let (tx, receipt): (Option<Transaction>, Option<send::receipt::Receipt>) = tokio::try_join!(
        rpc::call(ctx, "eth_getTransactionByHash", json!([id])),
        send::receipt::read(ctx, id)
    )?;
    let (Some(tx), Some(receipt)) = (tx, receipt) else {
        return Ok(None);
    };
    if tx.hash != id
        || tx.to != m.router
        || tx.from == m.order.user
        || receipt.status != U64::from(1)
        || receipt.transaction_hash != Some(id)
        || receipt.block_hash != Some(tx.block_hash)
        || !m.order.present_in(m.router, &tx.input)
    {
        return Ok(None);
    }
    let h = white_state::head(ctx, &format!("{:#x}", receipt.block_number)).await?;
    if h.hash != tx.block_hash || U256::from(h.timestamp) > m.order.deadline {
        return Ok(None);
    }
    let code: Bytes = rpc::call(ctx, "eth_getCode", json!([m.router, h.selector()])).await?;
    if keccak256(code) != m.runtime || m.tokens.is_empty() {
        return Ok(None);
    }
    let at = h.selector();
    for t in &m.tokens {
        t.verify(ctx, &at, &[]).await?;
    }
    let proxies: Vec<_> = m
        .tokens
        .iter()
        .filter(|t| t.proxy.is_some())
        .map(|t| t.address)
        .collect();
    if !proxies.is_empty() {
        let changes: Vec<alloy::rpc::types::Log> = rpc::call(
            ctx,
            "eth_getLogs",
            json!([{"blockHash":h.hash,"address":proxies,
            "topics":[keccak256("Upgraded(address)".as_bytes())]}]),
        )
        .await?;
        if !changes.is_empty() {
            return Ok(None);
        }
    }
    let Some(fill) = fill::confirmed(&m.order, m.router, &receipt) else {
        return Ok(None);
    };
    send::canonical_receipt::verify(ctx, id, &receipt).await?;
    Ok(Some(Mined {
        hash: id,
        tx_nonce: tx.nonce.to(),
        fill,
        receipt,
    }))
}

pub(super) async fn last_valid(ctx:&Ctx,m:&Marker,tip:Head)->GatewayResult<u64>{
    let start = m.start;
    let mut last = tip.number.to::<u64>();
    let base = white_state::head(ctx, &format!("{start:#x}")).await?;
    if base.hash != m.start_hash || last < start {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Ryze: изменился исходный финализированный блок",
        ));
    }
    if U256::from(tip.timestamp) > m.order.deadline {
        let mut low = start;
        while low < last {
            let mid = low + (last - low).div_ceil(2);
            let b = white_state::head(ctx, &format!("{mid:#x}")).await?;
            if U256::from(b.timestamp) <= m.order.deadline {
                low = mid;
            } else {
                last = mid - 1;
            }
        }
        last = low;
    }
    Ok(last)
}

/// Фильтр по исходному отправителю и физическим токенам. Верхняя граница по timestamp ограничивает работу после долгого простоя.
pub async fn hashes(ctx: &Ctx, m: &Marker, tip: Head) -> GatewayResult<BTreeSet<B256>> {
    let mut start=m.start;
    let last=last_valid(ctx,m,tip).await?;
    let topics = json!([
        IntentSwapExecuted::SIGNATURE_HASH,
        m.order.user.into_word(),
        m.order.tokenIn.into_word(),
        m.order.tokenOut.into_word()
    ]);
    let mut result = BTreeSet::new();
    while start <= last {
        let end = start.saturating_add(999).min(last);
        let logs: Vec<alloy::rpc::types::Log> = rpc::call(
            ctx,
            "eth_getLogs",
            json!([{"address":m.router,"topics":topics,
            "fromBlock":format!("{start:#x}"),"toBlock":format!("{end:#x}")}]),
        )
        .await?;
        for l in logs {
            if l.removed
                || l.address() != m.router
                || l.block_number.is_none_or(|n| n < start || n > end)
            {
                return Err(ctx.err(ErrorCode::BadResponse, "Ryze: событие вне заданной области"));
            }
            if let Ok(s) = IntentSwapExecuted::decode_log_data(&l.inner.data)
                && s.user == m.order.user
                && s.nonce == m.order.nonce
                && s.recipient == m.order.recipient
                && s.tokenIn == m.order.tokenIn
                && s.tokenOut == m.order.tokenOut
                && let Some(id) = l.transaction_hash
            {
                result.insert(id);
            }
        }
        if end == last {
            break;
        }
        start = end + 1;
    }
    Ok(result)
}

pub async fn unused_after_deadline(ctx: &Ctx, m: &Marker) -> GatewayResult<bool> {
    let fin = white_state::head(ctx, "finalized").await?;
    if fin.number.to::<u64>() < m.start || U256::from(fin.timestamp) <= m.order.deadline {
        return Ok(false);
    }
    let last=last_valid(ctx,m,fin).await?;
    let after=white_state::head(ctx,&format!("{:#x}",last.saturating_add(1))).await?;
    if after.number>fin.number || U256::from(after.timestamp)<=m.order.deadline{return Ok(false);}
    let at = after.selector();
    let (code, nonce): (Bytes, U256) = tokio::try_join!(
        rpc::call(ctx, "eth_getCode", json!([m.router, at])),
        canonical(ctx, &at, m.router, swapNoncesCall { user: m.order.user })
    )?;
    Ok(keccak256(code) == m.runtime && nonce == m.order.nonce)
}
