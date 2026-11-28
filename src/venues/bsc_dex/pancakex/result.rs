//! Плательщик газа, два физических перевода и привязка результата к позиции транзакции.
use super::{
    Terms,
    history::{self, Transaction},
    ledger::Marker,
    market::LABEL,
};
use crate::{
    core::types::{SendMethod, Side, TradeRequest, TradeResult, TradeStatus, TxOutcome},
    gateway::{
        errors::{ErrorCode, GatewayResult},
        settlement_intent as intent,
    },
    venues::bsc_dex::{exec::chain_tx, node::rpc, num, parts::Parts, quote::white_state, send},
};
use alloy_primitives::{B256, U64};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
pub(super) fn empty(r: TradeRequest, id: B256) -> intent::SpotFill {
    intent::SpotFill {
        trade: TradeResult {
            request: r,
            status: TradeStatus::NotFilled,
            filled_qty: Decimal::ZERO,
            avg_price: Decimal::ZERO,
            fees: vec![],
            venue_ref: id.to_string(),
            chain: None,
            finished_at: Utc::now(),
            executor: Some(LABEL.into()),
        },
        gross_quote: Decimal::ZERO,
    }
}
pub(super) async fn paid(
    p: &Parts,
    r: TradeRequest,
    m: &Marker,
    t: &Terms,
    id: B256,
) -> GatewayResult<intent::SpotFill> {
    let ctx = &p.ctx;
    let tk = &m.ticket;
    let c = &tk.cfg;
    let bad = || {
        ctx.err(
            ErrorCode::OutcomeUnknown,
            "PCSX: не подтверждены фактические выплаты и плательщик газа",
        )
    };
    let (tx, receipt) = tokio::try_join!(
        rpc::call::<Option<Transaction>>(ctx, "eth_getTransactionByHash", json!([id])),
        send::receipt::read(ctx, id)
    )?;
    let tx = tx.ok_or_else(bad)?;
    let receipt = receipt.ok_or_else(bad)?;
    if tx.hash != id
        || tx.from == tk.req.owner
        || receipt.status != U64::from(1)
        || receipt.block_hash != Some(tx.block_hash)
    {
        return Err(bad());
    }
    let h = white_state::head(ctx, &format!("{:#x}", receipt.block_number)).await?;
    if h.hash != tx.block_hash || history::expired(h.timestamp.to(), t.deadline) {
        return Err(bad());
    }
    let cash = t
        .cash_net(
            &receipt.logs,
            &[],
            t.roles.reactor == c.p.router.address,
            &c.output,
            tk.output_decimals(),
        )
        .ok_or_else(bad)?;
    let at = h.selector();
    tokio::try_join!(
        c.p.check(ctx, h, &tk.offer.version, t.roles.reactor),
        c.input.verify(
            ctx,
            &at,
            tk.req.owner,
            cash.filler,
            c.p.permit.address,
            None
        ),
        c.output.verify(
            ctx,
            &at,
            cash.filler,
            tk.req.owner,
            t.roles.reactor,
            Some(tk.output_decimals())
        )
    )?;
    let changes:Vec<alloy::rpc::types::Log>=rpc::call(ctx,"eth_getLogs",json!([{"blockHash":h.hash,"address":[c.p.router.address,tk.req.input,tk.req.output],"topics":[alloy_primitives::keccak256("Upgraded(address)".as_bytes())]}])).await?;
    if !changes.is_empty() {
        return Err(bad());
    }
    send::canonical_receipt::verify(ctx, id, &receipt).await?;
    send::canonical_receipt::observe(ctx, id, &receipt).await?;
    let (coin, stable) = if r.side == Side::Buy {
        (cash.received, cash.paid)
    } else {
        (cash.paid, cash.received)
    };
    let qty = num::to_decimal(coin, tk.decimals.0)
        .filter(|v| *v > Decimal::ZERO)
        .ok_or_else(bad)?;
    let gross = num::to_decimal(stable, tk.decimals.1).ok_or_else(bad)?;
    let tracked = send::track::Tracked {
        outcome: TxOutcome::Included,
        receipt: Some(receipt),
        cancel: None,
        unresolved: None,
    };
    let mut chain = chain_tx::chain_tx(
        id,
        tx.nonce.to(),
        vec![LABEL.into()],
        SendMethod::Single,
        &tracked,
        None,
    );
    chain.gas_bnb = Some(Decimal::ZERO);
    Ok(intent::SpotFill {
        trade: TradeResult {
            status: if qty < r.qty {
                TradeStatus::PartiallyFilled
            } else {
                TradeStatus::Filled
            },
            filled_qty: qty,
            avg_price: gross.checked_div(qty).ok_or_else(bad)?,
            fees: vec![],
            venue_ref: id.to_string(),
            chain: Some(chain),
            finished_at: Utc::now(),
            executor: Some(LABEL.into()),
            request: r,
        },
        gross_quote: gross,
    })
}
