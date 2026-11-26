//! Исход Hanji по хешу исходной подписи, транзакции, канонической квитанции и чистым ERC-20 Transfer.
use super::{
    execute::{self, Marker},
    wire::{Filled, Proof},
};
use crate::{
    core::types::{Fee, SendMethod, Side, TradeRequest, TradeResult, TradeStatus, TxOutcome},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        pending::PendingRef,
        settlement_intent as intent,
    },
    venues::bsc_dex::{exec, node::rpc, num, parts::Parts, resolve, send, wallet},
};
use alloy::{
    primitives::{Address, B256, Bytes, U64, U256, keccak256},
    sol_types::SolEvent,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
fn fill(proof: &Proof, r: &send::receipt::Receipt) -> Option<(U256, U256)> {
    let mut logs = r.logs.iter().filter(|l| {
        l.address() == proof.executor && l.topics().first() == Some(&Filled::SIGNATURE_HASH)
    });
    let f = Filled::decode_log_data(&logs.next()?.inner.data).ok()?;
    if logs.next().is_some()
        || (f.payer, f.operationId, f.market, f.input, f.output)
            != (
                proof.payer,
                proof.leg,
                proof.market,
                proof.input,
                proof.output,
            )
        || !proof.amounts_ok(f.spent, f.received, f.feeUnits)
    {
        return None;
    }
    let moved = |token, out| {
        if out {
            exec::fill::moved(r, token, Some(proof.payer), None)
        } else {
            exec::fill::moved(r, token, None, Some(proof.payer))
        }
    };
    let spent = moved(proof.input, true).checked_sub(moved(proof.input, false))?;
    let got = moved(proof.output, false).checked_sub(moved(proof.output, true))?;
    ((spent, got) == (f.spent, f.received)).then_some((spent, got))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tx {
    hash: B256,
    block_hash: B256,
    nonce: U64,
    from: Address,
    to: Address,
    input: Bytes,
    value: U256,
}
#[derive(Deserialize)]
struct Block {
    hash: B256,
    number: U64,
    transactions: Vec<B256>,
}
pub(in crate::venues::bsc_dex) async fn reconcile(
    p: &Parts,
    req: TradeRequest,
    pend: &PendingRef,
) -> GatewayResult<Option<TradeResult>> {
    let ctx = &p.ctx;
    let db = ctx.journal.db();
    let unknown = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "Hanji: исход сохранённой подписи ещё не подтверждён",
        )
        .pending(pend.clone())
    };
    let initial = pend
        .parse
        .clone()
        .and_then(|v| serde_json::from_value::<Marker>(v).ok());
    let k = match initial {
        Some(m) => m.key,
        None => execute::key(p, &req)?,
    };
    let entry = intent::load(db, &k)
        .await
        .map_err(|_| unknown())?
        .ok_or_else(unknown)?;
    let m: Marker = serde_json::from_value(entry.lookup.parse.clone().ok_or_else(unknown)?)
        .map_err(|_| unknown())?;
    let original = match &entry.operation {
        intent::Request::Order(r) => r,
        _ => return Err(unknown()),
    };
    let (hash, nonce) = resolve::keys(p, &entry.lookup)?;
    let nonce = nonce.ok_or_else(unknown)?;
    let o = &m.order;
    if ctx.chain_id() != Some(8453)
        || ctx.account.as_deref() != Some(m.account.as_str())
        || wallet::signer::address(ctx)? != o.payer
        || m.key != k
        || o.leg != keccak256(k.as_bytes())
        || pend.venue_ref != entry.lookup.venue_ref
        || pend.nonce.is_some_and(|n| n != nonce)
        || req.venue != ctx.venue()
        || req.deal_id != original.deal_id
        || (req.side, req.qty, req.asset.contract, &req.asset.chain)
            != (
                original.side,
                original.qty,
                original.asset.contract,
                &original.asset.chain,
            )
        || req.venue != original.venue
        || (!m.outer && k != execute::key(p, &req)?)
    {
        return Err(unknown());
    }
    let network: U64 = rpc::call(ctx, "eth_chainId", json!([])).await?;
    if network.to::<u64>() != 8453 {
        return Err(unknown());
    }
    let Some(r) = send::receipt::read(ctx, hash).await? else {
        return Ok(None);
    };
    let (b, tx, tip): (Block, Option<Tx>, U64) = tokio::try_join!(
        rpc::call(ctx, "eth_getBlockByNumber", json!([r.block_number, false])),
        rpc::call(ctx, "eth_getTransactionByHash", json!([hash])),
        rpc::call(ctx, "eth_blockNumber", json!([]))
    )?;
    let tx = tx.ok_or_else(unknown)?;
    let index = r
        .transaction_index
        .and_then(|i| usize::try_from(i).ok())
        .ok_or_else(unknown)?;
    if r.transaction_hash != Some(hash)
        || r.block_hash != Some(b.hash)
        || b.number != r.block_number
        || b.transactions.get(index) != Some(&hash)
        || tx.block_hash != b.hash
        || tx.hash != hash
        || tx.from != o.payer
        || tx.to != o.executor
        || tx.nonce != U64::from(nonce)
        || !tx.value.is_zero()
        || o.data().as_ref() != Some(&tx.input)
    {
        return Err(unknown());
    }
    let depth = ctx.int("send.replaced_confirm_blocks")?;
    if tip.to::<u64>() < r.block_number.to::<u64>().saturating_add(depth) {
        return Ok(None);
    }
    let ok = r.status == U64::from(1);
    if !ok && r.status != U64::ZERO {
        return Err(unknown());
    }
    let tracked = send::track::Tracked {
        outcome: if ok {
            TxOutcome::Included
        } else {
            TxOutcome::Reverted
        },
        receipt: Some(r.clone()),
        cancel: None,
        unresolved: None,
    };
    let chain =
        exec::chain_tx::chain_tx(hash, nonce, m.channels, SendMethod::Single, &tracked, None);
    if !ok {
        if !m.outer {
            intent::archive_attempt(
                db,
                &k,
                &entry,
                "Каноническая квитанция Hanji подтвердила откат",
            )
            .await
            .map_err(|_| unknown())?;
        }
        resolve::release_if_ours(p, hash, Some(nonce), false).await;
        return Err(ctx
            .err(
                ErrorCode::SwapReverted,
                "Hanji: подтверждён откат исходной транзакции",
            )
            .with_tx(chain));
    }
    let (spent, received) = fill(o, &r).ok_or_else(unknown)?;
    let (units, cash) = if req.side == Side::Buy {
        (received, spent)
    } else {
        (spent, received)
    };
    let qty = num::to_decimal(units, m.decs.0).ok_or_else(unknown)?;
    let gross = num::to_decimal(cash, m.decs.1).ok_or_else(unknown)?;
    if qty != req.qty {
        return Err(unknown());
    }
    let fee = chain.gas_bnb.ok_or_else(unknown)?;
    let trade = TradeResult {
        status: TradeStatus::Filled,
        filled_qty: qty,
        avg_price: gross.checked_div(qty).ok_or_else(unknown)?,
        fees: Fee::single(fee, "ETH"),
        venue_ref: hash.to_string(),
        chain: Some(chain),
        finished_at: Utc::now(),
        executor: Some("Hanji".into()),
        request: req,
    };
    if !m.outer && entry.result.is_none() {
        let outcome = intent::Outcome::Order(Box::new(intent::SpotFill {
            trade: trade.clone(),
            gross_quote: gross,
        }));
        intent::confirm(db, &k, &entry, outcome)
            .await
            .map_err(|_| unknown())?;
    }
    resolve::release_if_ours(p, hash, Some(nonce), false).await;
    intent::note_gross(Some(gross));
    Ok(Some(trade))
}
